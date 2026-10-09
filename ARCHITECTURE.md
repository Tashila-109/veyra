# Veyra architecture

## Executive summary

Veyra is one native macOS chat window backed by an orchestration actor and Claude Code's stream-json process.
SQLite rows are the source of truth for projects, threads, turns, transcript items and the default permission mode.
The engine actor is the sole database writer after startup.
The gpui `Shell` owns window state and a read-only event projection, not another database connection.
The engine never depends on gpui.

Claude sessions, readiness probes, replay fixtures and debug recording are implemented.
Codex and Grok have placeholder readiness but no codec.
There is no PTY, tool server, approvals/questions UI, interrupt ladder, startup process reaper, keep-awake guard or notification banner.

### System architecture

```text
User -> macOS window -> Shell + Theme + textarea + MessageScroller
                            | commands / acknowledgements
                            | subscription / fenced snapshots
main -> tokio runtime -> Engine actor -> veyra.db
                              | spawn / controls
                              v
                         Claude session -> owned CLI process group
                              | normalized events
                              +--------------------> Engine actor
Shell -> runtime readiness probes -> initialize-only CLI process

debug ctl -> private Unix socket -> socket thread -> gpui window actions
                                         debug session -> recorder / fake CLI
```

### Dependency hierarchy

```text
veyra -> veyra-engine -> rusqlite (bundled) + tokio
                     -> serde + serde_json + uuid + thiserror + tracing + regex
                     -> platform -> libc + Rust standard library
veyra -> gpui-kit + gpui-pre
      -> tokio + serde + serde_json + tracing + tracing-subscriber + anyhow
```

Direct versions are exact pins and `Cargo.lock` is kept.
The engine's tokio features are `sync`, `rt-multi-thread`, `time`, `macros`, `process` and `io-util`.
Only the application crate declares gpui dependencies.
`crates/engine/Cargo.toml` and `platform_boundary` enforce those boundaries.

## Startup and data ownership

`main::run` handles arguments before creating folders or starting gpui.
No argument opens the window; debug builds route `ctl` to its client.
Other arguments, including `mcp`, print `usage: veyra [mcp]` and exit 2.

`DataDir::resolve` reads `VEYRA_DATA_DIR`, or uses `~/Library/Application Support/Veyra`.
Relative overrides become absolute paths.
It validates the longest socket path before creating the data folder, `logs/` and `run/` at mode 0700.
The platform limit is 103 usable socket-path bytes.
Debug builds validate `control.sock`; release builds validate `veyra.sock` but create no service listener.

`Engine::start` locks the folder, opens `veyra.db` through a private 0600 file, migrates, reconciles stored work, then starts the actor on a dedicated `std::thread`.
The actor keeps the lock file alive.
`veyra.lock` is mode 0600 and its exclusive-lock retry lasts four seconds.
Only the lock holder stamps its pid or removes a stale debug socket.
A second launch refuses the folder and exits 1.
The OS releases the lock on exit or crash.
Startup does not signal orphan provider processes.

`main` keeps a multi-thread tokio runtime alive across the gpui loop.
The actor enters that runtime on its own thread; the UI never enters it.
The UI awaits runtime-agnostic tokio channels in gpui tasks and uses gpui timers.
Readiness work is explicitly spawned on the runtime through its handle.

After engine startup, `main` opens `logs/veyra.log` privately and installs INFO tracing.
It does not read `RUST_LOG`, so that variable cannot suppress readiness.
The debug listener binds before the window opens.
`Window::on_next_frame` prints `veyra ready pid=PID data_dir=DIR` to stderr and the log after the first frame.
`build.rs` embeds Git HEAD, or `unknown` outside Git, and watches HEAD, its reflog, packed refs and the current loose ref.
Quit enqueues engine shutdown and calls `cx.quit()` immediately; there is no graceful quit deadline yet.
macOS termination does not promise Rust destructors, so verification explicitly cleans run-owned provider groups.

## Actor decisions, storage and recovery

`Actor` owns the SQLite connection, projection rows, live-thread map, item-key map, text buffers and per-thread event counters.
It receives typed messages through a 256-slot tokio channel.
A timer sends Tick every five seconds; text uses separate flush and write messages.
The actor never waits for provider I/O.

`commands::decide` reads rows and runtime state, then produces a `ChangeSet` of row changes, events and effects.
Projection indexes identify each thread's turn and item ids; command overlays read those keyed rows rather than scanning every stored turn or item.
The actor completes buffered text for ended turns and finalizes newly settled task summaries before opening one transaction.
It calls `store::apply`, commits, installs the changed rows/runtime, publishes numbered events, then dispatches effects.
Row-bearing events resolve from the final changed rows rather than being patched individually.
StartTurn effects name the turn until install builds its text from committed items and dispatches its control.

Text coalesces per item, normally flushing to the UI at most once per 16 ms.
Requests, item completion and turn completion flush pending UI text immediately.
Unpersisted text writes every two seconds and at item/turn completion.
Completed items and ended turns free their item keys and text buffers.
A command write failure returns an error without publishing the decision.
A provider write failure retains changes, interrupts live sessions, refuses commands and publishes StorageBlocked; Tick retries before draining deferred process events.
Full and closed command queues are distinct errors; Busy carries a typed WaitReason and a closed request is distinct from either.
The UI shows command and acknowledgement failures as notices.

`LiveThread` owns an epoch outside its runtime variants, capabilities and the background-work flag.
Starting has no session handle until spawn returns one.
Live state is derived from the runtime variant, running turn and background-work flag.
Spawn increments the epoch; stale session events are ignored.
Turn events must also match the running turn, unless uncorrelated content starts or joins a Provider-origin turn.
Idle structured sessions park after ten minutes without a running turn, pending request or background work.
The existing engine supports holds, queue reasons, cascaded Stop/Archive, wake delivery and task settlement, but those controls/cards have no M2 UI.

`store::MIGRATIONS` owns schema 1, with WAL, `synchronous=NORMAL`, foreign keys and a two-second busy timeout.
Migrations use `PRAGMA user_version` in transactions and refuse newer schemas.
The eight tables are projects, threads, turns, items, requests, grants, tasks and settings.
User text lives once in an item referenced by its turn.
An ended turn carries both its optional start time and end time, including after load; there is no separate start-time map.
`store::apply` writes both live pid and group on Ready, clears both when parked, and preserves the first archive time on later archived writes.
Derived statuses and live state are not persisted as an event log.

Lineage CHECKs and the parent/root foreign key refuse grandchildren and inconsistent roots.
The hold CHECK allows only the typed hold reasons or null.
The singleton settings row starts in Full access and constrains the three modes.
Thread configuration remains in `config_json`; a default-mode change never rewrites existing threads.

`reconcile::decide` runs once, transactionally, before commands are accepted.
It interrupts stored running turns, expires requests and holds affected queues as Restarted without replacing an earlier hold.
It replaces an already-started unaccepted wake, preserves an unstarted wake, and leaves running child tasks open.
Terminal view returns to Chat with import_pending.
No provider work restarts automatically and Veyra writes no user CLI configuration.

## Claude process boundary

`provider/mod.rs` exposes typed controls and normalized events to the actor, not Claude wire types.
`ProviderKind::spawn` dispatches Claude to `session`; Codex, Grok and terminal spawn return NotBuilt.
Scripted sessions exist only in engine tests and start no OS process.

`ChildEnv` shares one login-shell PATH snapshot, with inherited PATH as fallback.
Engine::start initializes it on the startup caller before launching the actor, so session dispatch never runs a blocking login shell on the actor.
Resolution tries inherited PATH, that snapshot, then `KNOWN_BIN_DIRS`; executable checks and process-group mechanics are platform functions.
Every owned child has piped stdio, `kill_on_drop(true)`, its own process group and `VEYRA_PARENT_PID` naming the application.
It strips CLAUDECODE, CLAUDE_CODE_ENTRYPOINT, CLAUDE_CODE_SSE_PORT, CLAUDE_AGENT_SDK_VERSION and every inherited VEYRA_* variable before adding its own marker and optional replay fixture.
Veyra inherits the user's hooks/MCP configuration and never edits it.
The new-thread hint names that behavior.

One session task owns the child and a process-group guard.
Its select loop reads stdout lines, controls, output-channel closure and stdin-writer completion.
A dedicated task writes stdin, so a blocked pipe does not block the control loop.
The stderr task retains eight lines, each redacted and cut to 500 characters.
On an unfinished turn's stdout close, the task waits up to 500 ms for stderr, emits Failed with Veyra's crash text and tail, then Exited.
The group guard sends SIGKILL when the session task drops, not just to the leader.
A supervisor awaits its join handle and emits Exited with redacted panic text on failure.
The actor stores pid and group on Ready and parks on Exited.

`claude::Codec` owns stream-json envelopes, prompt correlation, native block keys, tool ids and background-work replacement.
It uses tolerant serde defaults and unknown variants, logs unknown notifications and refuses unsupported control requests with an error reply rather than approving them.
The first spawn uses a Veyra UUID with --session-id; later spawns use --resume.
Mode maps to default, acceptEdits or bypassPermissions plus --allow-dangerously-skip-permissions.
Each prompt has its own UUID.
A matched initialize reply makes the process ready to accept that first prompt; system/init later supplies the native id, CLI version and capabilities.
This avoids waiting for system/init before sending the prompt that causes Claude to emit it.

Replayed user frames and matching command_lifecycle frames acknowledge prompts.
Explicit result UUID echoes decide ownership.
Without an echo, a non-human-origin result is foreign only once echo mode is known or a lifecycle acknowledgement has arrived; unknown mode assigns the first result to Veyra's one pending prompt.
Result errors, stop reasons and terminal reasons decide the outcome, not subtype alone.
Aborted terminal reasons become Interrupted; incomplete/error reasons become Failed.
Content-block deltas and final assistant messages share keys to avoid duplicated streamed text.
Item completion is normalized separately from turn completion.
Each background_tasks_changed replaces the pending-work set, excluding ambient tasks.
Every new process starts with no background work pending.
Only the specific resume-not-found stderr class starts a fresh native session and adds a notice; other errors retain the native id for Retry.

## Readiness, fixtures and recording

Startup, Check again and Retry probe all providers on the runtime.
Claude resolves its path, reads its version, launches stream-json with no session flag, sends initialize and matches the request_id without a turn.
It reads models and optional account, then kills and waits for the probe process.
A missing binary is Not installed.
No signed-out signal has been captured: missing/empty account or another inconclusive probe error stays Ready, with the failed-thread view as fallback.
Codex and Grok are always Ready placeholders and run no probe command.
The Shell holds raw probe account metadata in memory but omits it from dump.

`fixture.rs` defines Veyra's transcript_start, expect_outbound, emit_inbound and exit records.
Headers name provider, protocol, CLI version, scenario and source.
The matcher compares JSON exactly except <any>, <cwd> and first-use <id:name> bindings.
emit_inbound supports after_ms; replies expand the same bindings.
`veyra-fake-cli` consumes one fixture and exits 3 with expected/actual diagnostics on mismatch.
Capture-derived fixtures come from Veyra's P3 and M2 captures; other cases are Veyra's scripted wires.

Debug `ChildEnv::load` is the only application read site for VEYRA_CLAUDE_BIN, VEYRA_FAKE_FIXTURES and VEYRA_RECORD.
The override replaces Claude's binary.
The engine registers new threads in creation order for a run; each thread's spawn E uses tN-sE.jsonl, with probe.jsonl for probes.
Stored threads register when first spawned in that run.
Missing replay files fail spawn with their path; the child gets the absolute singular VEYRA_FAKE_FIXTURE only in debug builds.
The standalone fake executable reads that singular variable.

The debug Recorder creates private 0600 captures through platform::private_file.
It binds cwd, known ids and the session token, replaces credential fields, redacts secret-shaped text and replaces account metadata with a placeholder.
It records inbound/outbound wires and exit, updating the header's CLI version from system/init.
The shared crash redactor applies exact token, Bearer word, credential-name value, long sk-/xai- word and URL password matches in that order.
The session has no MCP token in this phase.
Recorder and override read sites are compile-time debug-only; release string checks prove their exclusion from the application binary.

## Window projection, transcript and Settings

`Shell` owns Engine/Handle, AppState, focus, sidebar width/gesture, selected project/thread, view, draft configuration, probes, textarea, scroller, expanded rows, branch and notice.
It subscribes before requesting snapshots.
The subscription starts with SettingsChanged, projects, threads and turn rows; initial thread rows advance that thread's counter.
The gpui pump drains every queued event into one entity update, applies it and notifies once.
AppState counts applied batches and the largest measured gap between text-bearing batches.
A theme-cadence task refreshes elapsed Working time without a tokio timer on the UI thread.

Opening a transcript requests an actor snapshot, never SQLite from the UI.
While it is pending, AppState fences that thread's item events, retaining them and updating global thread metadata.
It installs stored items plus buffered text and replays only item events above the snapshot counter.
Later stale transcript events are ignored, but metadata stays on the ordered event stream: an items-only snapshot does not carry native-id, turn or live-state rows and cannot fence those updates.
This joins the item snapshot and event stream without duplicated or missing text.

The sidebar reads projects and thread statuses, and all resizing paths dispatch SetSidebarWidth.
The header reads title, status/elapsed time, project and a background Git branch lookup.
The centred transcript uses MessageScroller and selectable Markdown TextView for assistant text, a still stream caret, collapsed reasoning/tools and notices.
Tool output is bounded to its last 10,000 characters by the actor.
The failed view shows the cause, next step, Retry and Open Terminal on the project folder.
Retry probes again, clears a stopped hold when necessary and sends the prior prompt as a new turn.

The composer uses TextareaState and provider/model/effort/permission chips.
Not-ready providers disable Send with their repair command.
CreateThread sends the draft configuration; subsequent Send targets the selected thread.
Enter submits, Shift+Enter inserts a newline, and the textarea propagates Command+Enter to the bound Send action without a second subscription send.
New thread resets text and reads the current stored default.
Settings occupies the main column, opens through OpenSettings/Menu/Command-comma and exposes one three-mode row.
SetDefaultMode publishes SettingsChanged; Escape or thread/project selection leaves Settings.

`ui/actions.rs` owns actions, keys and the native menu.
The Theme global owns all colours, geometry, type, radii and motion tokens.
Theme::apply projects them through the kit and follows macOS appearance.
Violet is restricted to interaction roles; selected controls and hover remain neutral.
Geist and Geist Mono are bundled from Vercel's official release with its OFL beside them.
Traffic-light position is a point.
The requested window is 1280 by 800 with a 760 by 560 minimum; a window manager can override it.

## Debug protocol and verification trust

The control module, ctl argument, socket references and UI dump/input methods exist only under cfg(debug_assertions).
The driver uses DATA/control.sock at mode 0600 inside the private directory.
It trusts same-user clients and has no additional authentication.
One socket thread serves tagged JSON-line commands serially; a gpui task dispatches through the window.
Action dispatch is deferred by gpui; its reply acknowledges dispatch, and verification polls the resulting window state.
Lines are limited to 65536 bytes, socket reads/writes and UI-reply waits to two seconds, and the request queue to 16.
An incremental same-user sender can keep a line read occupied inside the per-read timeout.
A full queue rejects before enqueueing; a reply timeout does not cancel an action already queued.

Action accepts optional JSON data, keys parses before dispatch, text uses the focused textarea's public input handler, and quit dispatches Quit.
Dump reads live identity, theme, view, sidebar/status rows, selected native id/configuration/failure, items, composer, readiness, settings and pump counters.
The driver adds the engine's schema_version without opening the database.
Wait is client-only: it polls dump every 50 ms, for 30 seconds by default, and exits 0 for status, 1 for timeout, 2 for bad usage and 3 for a missing/archived thread.
No wait server command exists.

verify-veyra launches only its isolated fake lane by default and keeps proof outside scratch.
Cleanup first reads live_pgid, stops only run-owned groups/pids, checks live effective-user markers through Darwin sysctl without printing arguments, then removes scratch.
The fake lane cannot create privileged processes; inaccessible root-effective processes are outside that marker scan.
Release proof uses a disposable HOME and restricted PATH to avoid running an unauthorized live Claude probe.
The platform module owns macOS file modes, locks, sockets, login-shell PATH, executable checks, group setup/signals and Terminal opening.
Its selection rejects unsupported targets with compile_error.
The textual platform_boundary test is a guard, not proof against every indirect OS call; review still checks indirect commands and strings.

## Source map

- [Entry and runtime](crates/veyra/src/main.rs), [engine handle](crates/engine/src/lib.rs), [data folder](crates/engine/src/data_dir.rs).
- [Actor](crates/engine/src/actor.rs), [decisions](crates/engine/src/commands.rs), [domain/indexes](crates/engine/src/domain.rs), [live variants](crates/engine/src/live.rs).
- [Storage/schema](crates/engine/src/store.rs), [recovery](crates/engine/src/reconcile.rs), [events/snapshots](crates/engine/src/events.rs).
- [Provider boundary](crates/engine/src/provider/mod.rs), [environment](crates/engine/src/provider/env.rs), [sessions/probes](crates/engine/src/provider/session.rs), [Claude codec](crates/engine/src/provider/claude.rs).
- [Fixture format](crates/engine/src/fixture.rs), [fake CLI](crates/engine/src/bin/veyra-fake-cli.rs), [recording/redaction](crates/engine/src/record.rs).
- [Window/pump/fence](crates/veyra/src/ui/mod.rs), [transcript](crates/veyra/src/ui/transcript.rs), [composer](crates/veyra/src/ui/composer.rs), [Settings](crates/veyra/src/ui/settings.rs), [actions](crates/veyra/src/ui/actions.rs), [theme](crates/veyra/src/ui/theme.rs).
- [Platform mechanisms](crates/engine/src/platform/macos.rs), [selection](crates/engine/src/platform/mod.rs), [boundary guard](crates/engine/src/platform/boundary.rs).
- [Driver](crates/veyra/src/control.rs), [name coverage](crates/veyra/src/verify_map.rs), [verification skill](.agents/skills/verify-veyra/SKILL.md).

## Verification

Workspace tests cover private paths/modes, launch locking, platform_boundary, all 66 table_ command cells, recovery/storage/session behavior and theme contrast.
The eleven replay_claude tests use the real session task with the fake CLI for text/tools, correlation, resume, crash/error, background replacement and readiness.
record_redaction covers token removal, private captures, version/account sanitization and a fixture secret-shape sweep.
crash_text_bounds_and_redacts, claude_args_per_mode, terminal_reason_overrides_success_subtype, snapshot_fence_applies_only_newer_events, snapshot_fence_keeps_metadata_not_carried_by_snapshot, ctl_wait_exit_codes and verify_map_names guard the new boundaries.
Clippy with -D warnings and cargo fmt --check check source quality.
The four verify-veyra pages prove the real window, launch/relaunch row preservation, readiness without threads, Working-to-Done streaming, failure/Retry, default-mode persistence and cleanup.
The release page scans driver/override strings with a debug positive control and runs a socket-free release window.
The ignored live test requires fresh human consent; it is not a workspace-test live invocation.
No live background-task signal or signed-out account shape was captured.
Permission/approval behavior, Stop/interrupt continuation, native keyboard/IME/pointer delivery and Open Terminal's OS side effect are not proved by fake replay or the debug driver.
