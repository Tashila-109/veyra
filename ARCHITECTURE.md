# Veyra architecture

## Executive summary

Veyra is one native macOS chat window backed by an orchestration actor and Claude Code's stream-json process.
SQLite rows are the source of truth for projects, threads, turns, transcript items and the default permission mode.
The engine actor is the sole database writer after startup.
The gpui `Shell` owns window state and a read-only event projection, not another database connection.
The engine never depends on gpui.

Claude sessions, readiness probes, replay fixtures and debug recording are implemented.
Codex and Grok have placeholder readiness but no codec.
There is no PTY, tool server, approvals/questions path (codec request mapping, session answer or UI), startup process reaper, keep-awake guard or notification banner.

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
Shell -> runtime branch task -> git child

debug ctl -> private Unix socket -> socket thread -> gpui window actions
                                         debug session -> recorder / fake CLI
```

### Dependency hierarchy

```text
veyra -> veyra-engine -> rusqlite (bundled) + tokio
                     -> serde + serde_json + uuid + thiserror + tracing + regex + base64
                     -> platform -> libc + Rust standard library
veyra -> gpui-kit + gpui-pre
      -> tokio + serde + serde_json + tracing + tracing-subscriber + anyhow
```

Direct versions are exact pins and `Cargo.lock` is kept.
The engine's tokio features are `sync`, `rt-multi-thread`, `time`, `macros`, `process` and `io-util`.
Only the application crate declares gpui dependencies.
`platform_boundary` scans for libc, socket, process-group, target and listed command strings outside `platform/`; the header's `git` child is a reviewed exception.
Manifest review keeps gpui out of the engine and direct versions exact; no automated manifest check enforces those rules.

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
The login-shell PATH read runs synchronously before the window in its own process group.
`platform::bounded_shell_path` stops that group after two seconds and returns an error; `ChildEnv` logs the error and falls back to inherited PATH.
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
Quit cancels the branch task, enqueues engine shutdown and calls `cx.quit()` immediately; Quit does not wait for providers.
A provider process group running at Quit can outlive the app.
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
`Actor::write` opens no transaction when the change holds no row or live-state changes.
It still installs text buffers and publishes events/effects, so pure streaming deltas do not require a SQLite write.
Row-bearing events resolve from the final changed rows rather than being patched individually.
StartTurn effects name the turn until install builds its text from committed items and dispatches its control.

The send-to-reply path is ordered as follows.

1. CreateThread stores the thread and user item with a queued turn.
2. The Spawn effect starts the session on its own process group; an exit before Ready fails the oldest queued turn and holds the thread as Stopped.
3. A matched initialize reply or system/init publishes Ready and changes Starting to Structured.
4. start_changed makes the queued turn Running and dispatches StartTurn with committed prompt text.
The first prompt waits for Ready.
The codec also emits Ready on system/init, so an early system/init can open this gate before the initialize reply.
5. The codec normalizes deltas; the actor buffers text and counts actual stream deltas before flushing.
6. Item and turn completion persist final rows, free streaming buffers and publish the engine-owned status.

Text coalesces per item, normally flushing to the UI at most once per 16 ms.
Requests, item completion and turn completion flush pending UI text immediately.
Unpersisted text writes every two seconds and at item/turn completion.
Completed items and ended turns free their item keys and text buffers.
A command write failure returns an error without publishing the decision.
A provider, Tick or text write failure retains changes, interrupts live sessions, refuses commands and publishes StorageBlocked.
Tick retries before draining deferred process events.
Full and closed command queues are distinct errors; Busy carries a typed WaitReason and a closed request is distinct from either.
The UI shows command and acknowledgement failures as notices.

`LiveThread` owns an epoch outside its runtime variants, capabilities and the background-work flag.
Starting receives its session handle before the task launches its child.
The engine derives thread status with `domain::thread_status` and publishes `ThreadStatusChanged` after changed thread, turn, request or live rows.
The window, sidebar, header, dump and ctl wait consume that status, including background work, NeedsYou and InTerminal.
Spawn increments the epoch; stale session events are ignored.
Turn events must also match the running turn, unless uncorrelated content starts or joins a Provider-origin turn.
An admitted `TurnEnded { park: true }` first sends Shutdown and enters `Stopping { Park }`, even if its turn already ended.
That retires the session epoch for turn admission; the actor waits for Exited before parking and spawning eligible queued work.
A late end cannot settle another turn.
Idle structured sessions park after ten minutes without a running turn, pending request or background work.
The engine supports holds, queue reasons, cascaded Stop/Archive, Interrupt, Respond, wake delivery and task settlement.
Those controls and cards have no window UI.
On a Starting thread, Stop, Archive and Interrupt send Shutdown and wait in Stopping for Exited.
The session receives that control before stdout; if the child has not launched yet it stops the group once launch returns and emits no Ready.
A Ready from an already-running initialization is ignored after the actor leaves Starting.
On a running Structured thread, those controls end the engine turn with Interrupted and "Stopped by you", keep its session and send the protocol interrupt.
`Live::Structured.draining` keeps that ended turn's id until its own CLI end arrives.
Send or Continue queues new work with `WaitReason::Stopping` during the drain.
An uncorrelated tail from a drained Provider-origin turn is logged and dropped before it can open another turn.
The old CLI end clears the drain; the same change applies the start rule, which still refuses Stopping sessions.
Stop and Archive also discard queued turns and set a Stopped hold, while Interrupt keeps them and sets an Interrupted hold.
All three preserve an existing hold.
The scripted test session ends silently on channel close, so engine tests observe only Stopping.
Tasks cannot arise through the application because the tool server is not implemented.
Requests cannot arise either: the Claude codec answers every control request with an error and emits no RequestOpened, and the session discards Answer controls, so Respond and NeedsYou are reachable only in scripted engine tests.

`store::MIGRATIONS` owns schema 2, with WAL, `synchronous=NORMAL`, foreign keys and a two-second busy timeout.
Migrations use `PRAGMA user_version` in transactions and refuse newer schemas.
The eight tables are projects, threads, turns, items, requests, grants, tasks and settings.
User text lives once in an item referenced by its turn.
An ended turn carries both its optional start time and end time, including after load; there is no separate start-time map.
Migration 2 normalizes older empty CLI versions to NULL once.
Loading rows is read-only and treats any remaining empty CLI version as unknown.
Empty versions become unknown at the codec, probe and row-load boundaries; the actor and writes trust that input.
`store::apply` writes both live pid and group on Ready, clears both when parked, and preserves the first archive time on later archived writes.
Derived statuses and live state are not persisted as an event log.
After a crash, stored live pid and group stay stale until that thread next spawns or parks; startup has no process reaper.

Lineage CHECKs and the parent/root foreign key refuse grandchildren and inconsistent roots.
The hold CHECK allows only the typed hold reasons or null.
The singleton settings row starts in Full access and constrains the three modes.
Thread configuration remains in `config_json`; a default-mode change never rewrites existing threads.

`reconcile::decide` runs once before commands are accepted.
The actor routes that change through `prepare_commit`, completing interrupted text and unfinished tools in the same startup transaction.
It interrupts stored running turns, expires requests and holds affected queues as Restarted without replacing an earlier hold.
It replaces an already-started unaccepted wake, preserves an unstarted wake, and leaves running child tasks open.
Terminal view returns to Chat with import_pending.
No provider work restarts automatically and Veyra writes no user CLI configuration.

## Claude process boundary

`provider/mod.rs` exposes typed controls and normalized events to the actor, not Claude wire types.
The actor dispatches through `Spawner::spawn`.
Its `Cli` variant sends Claude to `session` with the Engine's shared `ChildEnv`; Codex and Grok return NotBuilt.
`Spawner::terminal` returns NotBuilt.
Scripted sessions exist only in engine tests and start no OS process.

`ChildEnv` shares one login-shell PATH snapshot, with inherited PATH as fallback.
It retains the shell failure and reports it on load, including UI readiness loads after the app installs tracing.
Engine::start initializes it on the startup caller before launching the actor, so session dispatch never runs a blocking login shell on the actor.
Resolution tries absolute entries from inherited PATH, that snapshot, then `KNOWN_BIN_DIRS`, returning a canonical absolute executable path.
Relative and empty search entries are ignored.
Executable checks and process-group mechanics are platform functions.
Every provider and probe child has piped stdio, `kill_on_drop(true)`, its own process group and `VEYRA_PARENT_PID` naming the application.
It strips CLAUDECODE, CLAUDE_CODE_ENTRYPOINT, CLAUDE_CODE_SSE_PORT, CLAUDE_AGENT_SDK_VERSION and every inherited VEYRA_* variable before adding its own marker and optional replay fixture.
Veyra inherits the user's hooks/MCP configuration and never edits it.
The new-thread hint names that behavior.

One session task owns the child and a process-group guard.
Its select loop reads stdout bytes, controls, the interrupt deadline, output-channel closure and stdin-writer completion.
A dedicated task writes stdin, so a blocked pipe does not block the control loop.
`ByteLines` buffers at most 4 MiB per stdout or stderr line and retains partial reads across control-loop cancellation.
It decodes accepted bytes lossily and discards oversized lines through their newline.
An oversized stdout line logs its length and emits one notice; invalid JSON logs its length and is skipped without ending the session.
The last rejected JSON diagnostic is retained for a later EOF failure, preserving the earlier crash contract.
The stderr task retains eight raw lines bounded by that same byte cap, replacing an oversized line with a length-only diagnostic.
Redaction sees each complete retained line before its display cut.
Streamed text and tool input remain unbounded; tool output is limited to its last 10,000 characters.
`with_stderr` redacts the failure and each tail line once, then cuts each redacted line to 500 characters.
Interrupt informs the codec and sends the protocol request, starts a three-second grace, then sends group SIGTERM and group SIGKILL one second later.
A CLI turn end cancels that ladder and leaves the process available for another turn.
When the loop ends, the task waits up to 500 ms for stderr and observes the leader's status with `try_wait` before cleanup signals it.
It then cleans the group, synthesizes a parked turn end if a turn remains active, and emits Exited.
An open interrupt takes precedence and synthesizes Interrupted with "Stopped by you".
Otherwise a latched assistant error supplies Failed, or an observed SIGHUP, SIGINT, SIGTERM, 129, 130 or 143 supplies Interrupted naming the signal or code.
SIGKILL and other outcomes stay Failed with the loop diagnostic or Veyra's bounded, redacted crash text.
Only an externally observed status is classified; a cleanup-generated SIGTERM cannot make a running turn read Interrupted.
Exited follows group cleanup and its leader's reap, including an initialize-send failure.
On Exited, the actor ends any remaining running turn as Failed if crash text exists, otherwise Interrupted.
While the thread is still Starting, every exit before Ready fails the oldest queued turn with its failure or Veyra's crash text and holds the thread as Stopped, including a clean exit, an unknown status or stdout closing while the process remains alive.
If an exit before Ready carries no failure text, the actor uses "The session exited before it was ready"; only scripted sessions reach that fallback.
A spawn error never starts a process and is posted back as Exited, so every Codex or Grok thread fails this way.
After Ready, crash text with no running turn is dropped and any eligible queued turn respawns.
At normal run end, cleanup sends SIGTERM to the recorded owned group.
An earlier `try_wait` status check can already have reaped its leader.
Dropping a live guard immediately sends SIGKILL and schedules the same bounded cleanup.
Cleanup checks every 50 ms and waits up to one second for the group to exit.
It sends SIGKILL if the group remains alive or a child/group check fails, then waits for the leader.
The guard retains the child through that ladder, then waits up to one second for the group to disappear after reaping its leader.
If that deadline expires, it logs the pgid at WARN and returns so the turn end and Exited can arrive.
Surviving members remain untracked after parking clears the stored group; that warning is the only retained group record.
Cleanup preserves the first check error if a later check or the leader's reap also fails.
This also covers probe cancellation and resume fallback.
A supervisor awaits the session task and every registered group cleanup before emitting Exited with redacted panic text.
The actor stores pid and group on Ready and parks on Exited.

`claude::Codec` owns stream-json envelopes, prompt correlation, native block keys, tool ids and background-work replacement.
Serde defaults handle missing fields and unknown variants.
Unknown top-level frames, non-initialize control responses and unhandled system subtypes log their type and subtype at INFO.
Unknown stream events, deltas and content blocks log their type and category at INFO.
These logs never include frame content.
Foreign results are dropped silently; unsupported control requests get an error reply.
Lifecycle frames that neither acknowledge a pending prompt nor start a Provider-origin turn are dropped silently.
Unmatched user frames are also dropped silently.
Invalid JSON is skipped by the byte reader; type errors in valid frames or final blocks still end the process and fail its running turn.
Final blocks already delivered by the stream are skipped before parsing.
The first spawn uses a Veyra UUID with --session-id; later spawns use --resume.
Mode maps to default, acceptEdits or bypassPermissions plus --allow-dangerously-skip-permissions.
Ask first and Accept edits tool requests that need host approval currently receive the unsupported-request error, so those tools fail.
Each prompt has its own UUID.
A matched initialize reply makes the process ready with no CLI version.
A known stored version survives that provisional Ready until system/init replaces it.
The later system/init supplies native id, version and capabilities; a changed native id also adds a notice on the latest turn.
This avoids waiting for system/init before sending the prompt that causes Claude to emit it.

Replayed user frames and matching command_lifecycle frames acknowledge prompts.
Without a Veyra prompt, command_lifecycle queued/started or a stream message_start opens a Provider-origin turn, whichever arrives first.
Content without either start signal also opens that turn.
Explicit result UUID echoes decide ownership.
While a Veyra prompt is pending, a no-echo non-human result is foreign once echo mode is known or a lifecycle acknowledgement has arrived.
Unknown mode assigns the first result to the one pending prompt.
After that prompt ends, an uncorrelated result ends a running Provider-origin turn without changing the completed foreground turn.
An assistant's optional error latches Veyra's words on the active prompt; its own result ends Failed regardless of result subtype or error flag.
Authentication, organization, account-hold, verification, billing and cloud-credential errors also set park.
EOF keeps those words and parks the gone process.
An open Veyra interrupt overrides the error outcome while preserving park.
It suppresses unstreamed final assistant text blocks starting with `[ede_diagnostic]`; already-emitted stream deltas are unchanged.
Without an open interrupt or latched error, aborted terminal reasons take precedence over error subtype and become Interrupted.
Other error flags, error subtypes, max_tokens, refusal and incomplete terminal reasons become Failed.
Final assistant blocks count occurrences of each kind per message id across frames and match the stream's blocks in that order.
Prompt start, turn end and `message_start` replace the per-message stream state and reset those counts.
They reuse stream keys even when Claude omits empty reasoning from the final content list.
Item completion is normalized separately from turn completion.
Empty reasoning deltas and final reasoning blocks add no transcript item.
Each background_tasks_changed replaces the pending-work set, excluding ambient tasks.
Every new process starts with no background work pending.
Assistant, user and stream frames with parent_tool_use_id never open a Provider turn or create root text/tool events.
A pending Task or Agent call routes child text into ToolOutput for that root call; unknown and completed parents log only their type and are dropped.
The child codec matches streamed and final text without duplicating it, and whole user frames supply text and string tool-result content.
The actor appends child text to the stored ToolItem.output_tail, keeping its last 10,000 characters, and publishes the updated running row.
The result remains absent while the call runs; snapshots and UI events carry its tail.
A defaulted empty tail lets existing stored tool rows load without a schema migration.
ToolFinished combines that tail with the final output under the same cap and moves it into the result.
The current window renderer reads the result tail only, so the running tail is available to the window projection but is not yet rendered.
Compact-boundary, conversation-reset, API-retry, non-allowed usage-limit and model-fallback frames add notices in Veyra's words.
The reset notice explicitly says the CLI no longer sees the earlier messages.
Only the specific resume-not-found stderr class starts a fresh native session and adds a notice; other errors retain the native id for Retry.

## Readiness, fixtures and recording

Startup, Check again and Retry probe each provider independently on the runtime.
`Probes` owns each provider's cancellable task.
A replacement cancels and waits for the earlier task, including its process-group ladder, before starting.
Shell generation numbers reject stale probe replies.
There is no probe timeout; a hung Claude stays Checking while the other providers become Ready.
Claude resolves its path, reads its version, launches stream-json in the private data folder with no session flag, sends initialize and matches the request_id without a turn.
It reads models and optional account, then ends and waits for the probe process through the same group ladder.
A missing binary is Not installed.
The probe currently treats missing/empty account and other inconclusive metadata as Ready, with the failed-thread view as fallback.
It does not use account keys to determine signed-out readiness.
Codex and Grok are always Ready placeholders and run no probe command.
The Shell holds raw probe account metadata in memory.
Dump exposes only account presence as known or unknown, never its value.

`fixture.rs` defines Veyra's transcript_start, expect_outbound, emit_inbound, emit_raw and exit records.
Headers name provider, protocol, CLI version, scenario and source.
The matcher compares JSON exactly except <any>, <cwd> and first-use <id:name> bindings.
emit_inbound supports after_ms; replies expand the same bindings.
emit_raw decodes base64 bytes and writes them repeatedly for invalid-byte and oversized-line cases.
The fake CLI ignores SIGTERM when its header sets ignore_sigterm.
`veyra-fake-cli` consumes one fixture and exits 3 with expected/actual diagnostics on mismatch.
Capture-derived fixtures come from Veyra's own CLI recordings; other cases are Veyra's scripted wires.

The debug fields of `ChildEnv::load` are the only application read site for VEYRA_CLAUDE_BIN, VEYRA_FAKE_FIXTURES and VEYRA_RECORD.
The override replaces Claude's binary.
Each Engine owns its lane registry through its shared ChildEnv.
New threads register in creation order; each thread's spawn E uses tN-sE.jsonl, with probe.jsonl for probes.
Stored threads register when first spawned in that run.
Missing replay files fail spawn with their path; the child gets the absolute singular VEYRA_FAKE_FIXTURE only in debug builds.
The standalone fake executable reads that singular variable.

The debug Recorder creates private 0600 captures through platform::private_file.
It binds the supplied cwd, optional home folder and known ids and can bind a session token; sessions currently have no MCP token.
ChildEnv reads HOME once and supplies it to the recorder; `<home>` is a capture placeholder, not a matcher binding.
It replaces credential fields, redacts secret-shaped text, replaces hook output with a placeholder and keeps account keys with masked values.
The probe is recorded as probe.jsonl; a later probe replaces that capture only after the earlier probe has ended.
It records inbound/outbound wires and exit, updating the header's CLI version from system/init.
The shared crash redactor applies exact token, Bearer word, credential-name value, long sk-/xai- word and URL password matches in that order.
The fixture privacy sweep also rejects concrete home paths, email and account-id shapes, and a neutral planted needle in raw or assembled text.
Recorder and override read sites are compile-time debug-only; release string checks prove their exclusion from the application binary.

## Window projection, transcript and Settings

`Shell` owns the engine handle and read-only projection, window navigation and input, readiness tasks and transient presentation state.
`MainView::Chat` carries its thread id; Settings keeps an optional return thread.
It subscribes before requesting snapshots.
The subscription starts with SettingsChanged, projects, threads, turn rows and one status per thread.
It also sends StorageBlocked if storage is blocked.
Every thread-scoped initial row advances that thread's counter.
The gpui pump drains every queued event into one entity update, applies it and notifies once.
AppState counts applied batches and the largest measured gap between text-bearing batches.
A theme-cadence task refreshes elapsed Working time without a tokio timer on the UI thread.

Opening a transcript requests an actor snapshot, never SQLite from the UI.
Every snapshot request has an identity.
While it is pending, AppState fences that thread's item events, retaining them and updating global thread metadata.
Reselecting preserves buffered events; only the current request may install the items and fence.
An enqueue failure releases only that request's fence, replays buffered events and shows a load notice.
It installs stored items plus buffered text and replays only item events above the snapshot counter.
Snapshot installation and fence release do not await the header's Git branch lookup.
Later stale transcript events are ignored, but metadata stays on the ordered event stream: an items-only snapshot does not carry native-id, turn or live-state rows and cannot fence those updates.
This joins the item snapshot and event stream without duplicated or missing text.

The sidebar reads projects and thread statuses, and all resizing paths dispatch SetSidebarWidth.
The header reads title, status/elapsed time, project and a separate Git branch task on the tokio runtime.
Branch lookup starts once the selected Chat thread's cwd is known, either at selection or after an event batch supplies its row.
The retained task prevents another lookup until Chat is left or a thread is reselected.
That child inherits the app environment, has kill-on-drop and no timeout.
`Shell::show` and Quit call `forget_branch` to cancel the task and clear the displayed branch.
Dropping the Shell also cancels the task, and its reply checks the selection identity.
The centred transcript uses MessageScroller and selectable Markdown TextView for assistant text, a still stream caret, collapsed reasoning/tools and notices.
The actor trims stored tool output to its last 10,000 characters.
The failed view appears when the newest turn failed and shows the cause, next step, Retry and Open Terminal on the project folder.
Retry probes again; once the thread's provider is Ready it clears any hold and resends the newest user text in the transcript as a new turn.
If a second message was queued before Ready when the first turn failed, the thread shows Held without the failed view, and Retry clears the hold, so the queued second message runs and is then sent again, while the first prompt remains failed.
Send also clears a hold when the thread has no queued turn.

The composer uses TextareaState and provider/model/effort/permission chips.
Typed gpui action payloads reject malformed ids and enum values at dispatch, with a visible error.
On New thread, OpenProviderPicker and its key open the provider menu through a controlled kit popover.
An open menu rebuilds its readiness labels when probing begins and when each reply arrives.
Not-ready providers disable Send with their repair command.
CreateThread sends the draft configuration; subsequent Send targets the selected thread.
On an open thread, model, effort and permission chips send Configure, which is refused while work runs and parks an idle session.
Enter submits, Shift+Enter inserts a newline, and the textarea propagates Command+Enter to the bound Send action without a second subscription send.
New thread resets text and reads the current stored default.
Settings occupies the main column, opens through OpenSettings/Menu/Command-comma and exposes one three-mode row.
SetDefaultMode publishes SettingsChanged; Escape or thread/project selection leaves Settings.

`ui/actions.rs` owns actions, keys and the native menu.
The Theme global owns all colours, geometry, type, radii and motion tokens.
Theme::apply projects them through the kit and follows macOS appearance.
Violet marks focus, caret, selection, streaming and Working status; selected controls and hover remain neutral.
Geist and Geist Mono are bundled from Vercel's official release with its OFL beside them.
The requested window is 1280 by 800 with a 760 by 560 minimum; a window manager can override it.

## Debug protocol and verification trust

The control module, ctl argument, driver socket references and UI dump/input methods exist only under cfg(debug_assertions).
The driver uses DATA/control.sock at mode 0600 inside the private directory.
It trusts same-user clients and has no additional authentication.
One socket thread serves tagged JSON-line commands serially; a gpui task dispatches through the window.
Named actions dispatch through the window on the Shell's stable root focus; keys use the focused path.
Quit uses gpui's deferred Window::dispatch_action.
Replies acknowledge dispatch rather than completed engine work, so verification polls the resulting window state.
Lines are limited to 65536 bytes, socket reads/writes and UI-reply waits to two seconds, and the request queue to 16.
An incremental same-user sender can keep a line read occupied inside the per-read timeout.
A full queue rejects before enqueueing; a reply timeout does not cancel an action already queued.

Action accepts optional JSON data, keys parses before dispatch, text uses the focused textarea's public input handler, and quit dispatches Quit.
Dump reads identity, theme, view, engine status rows, selected native id/configuration/failure and rendered header status and branch, items, composer, readiness, settings and pump counters.
Debug-only item delta_count counts actual text-stream deltas in the current process and survives transcript snapshots; it is not persisted across app restarts.
Debug counts are removed when their items leave the projection or the thread is archived.
The driver adds the engine's schema_version without opening the database.
Wait is client-only and polls dump every 50 ms, for 30 seconds by default.
It exits 0 for status, 1 for timeout or a driver/connection error, 2 for bad usage and 3 for a missing or archived thread.
A timeout writes `veyra ctl wait timed out:` to stderr; a driver or connection failure writes `veyra ctl driver error:` with its cause.
No wait server command exists.

[verify-veyra](.agents/skills/verify-veyra/SKILL.md) owns the fresh-shell launch, driver, capture, evidence and cleanup procedure.
Its fake lane isolates data and uses only the fake CLI; an explicitly authorized live lane records actual provider wires.
Proof stays outside disposable scratch, and every action targets a process this run started.
Captures require the owned window to remain on-screen and frontmost; the guard prevents covered or stale frames from counting as proof.
Cleanup checks owned app pids, provider groups and same-user parent markers without printing arguments before removing scratch.
Release proof uses a disposable HOME and restricted PATH, scans driver/override strings and checks a socket-free window.
Only one owned Veyra window exists during that release check; it takes no PNG.
The platform module owns macOS file modes, locks, sockets, login-shell PATH, executable checks, group setup/signals and Terminal opening.
Its selection rejects unsupported targets with compile_error.
The textual platform_boundary test is a guard, not proof against every indirect OS call; review still checks indirect commands and strings.

## Source map

- [Entry and runtime](crates/veyra/src/main.rs), [engine handle](crates/engine/src/lib.rs), [data folder](crates/engine/src/data_dir.rs).
- [Actor](crates/engine/src/actor.rs), [decisions](crates/engine/src/commands.rs), [domain/indexes](crates/engine/src/domain.rs), [live variants](crates/engine/src/live.rs).
- [Storage/schema](crates/engine/src/store.rs), [recovery](crates/engine/src/reconcile.rs), [events/snapshots](crates/engine/src/events.rs).
- [Provider boundary](crates/engine/src/provider/mod.rs), [environment](crates/engine/src/provider/env.rs), [sessions/probes](crates/engine/src/provider/session.rs), [Claude codec](crates/engine/src/provider/claude.rs).
- [Fixture format](crates/engine/src/fixture.rs), [fake CLI](crates/engine/src/bin/veyra-fake-cli.rs), [recording/redaction](crates/engine/src/record.rs).
- [Window/pump/fence/branch](crates/veyra/src/ui/mod.rs), [transcript](crates/veyra/src/ui/transcript.rs), [composer](crates/veyra/src/ui/composer.rs), [Settings](crates/veyra/src/ui/settings.rs), [actions](crates/veyra/src/ui/actions.rs), [theme](crates/veyra/src/ui/theme.rs).
- [Platform mechanisms](crates/engine/src/platform/macos.rs), [selection](crates/engine/src/platform/mod.rs), [boundary guard](crates/engine/src/platform/boundary.rs).
- [Driver](crates/veyra/src/control.rs), [name coverage](crates/veyra/src/verify_map.rs), [verification skill](.agents/skills/verify-veyra/SKILL.md).

## Verification

Workspace tests cover private paths and modes, launch locking, platform_boundary, command-table behavior, recovery, storage, sessions and theme contrast.
replay_claude uses the real session task with the fake CLI for text and tools, prompt correlation, provider-turn completion, resume, crash and error outcomes, background replacement and readiness.
It also checks assistant-error parking, interrupt outcomes and ladder cancellation, subagent isolation against the live Agent recording, notices and malformed-byte recovery.
Actor tests cover retirement before queued work, the Stop drain, Provider-tail rejection and skipped text-only transactions.
Platform tests check the two-second login-shell bound and process exit classification.
record_redaction covers token removal, private captures, version/account sanitization and a fixture secret-shape sweep.
Tests guard failure redaction, launch arguments, terminal outcomes, snapshot ordering, metadata delivery, wait exits and driver-name coverage.
Application tests enable gpui test-support to exercise Shell selection and its event pump in a synthetic window.
Shell tests load the login-shell PATH as the app does, and run no provider CLI.
Clippy with -D warnings and cargo fmt --check check source quality.
The verify-veyra pages cover the real window, launch and relaunch row preservation, readiness without threads, streaming and background status, failure and Retry, default-mode persistence, header-branch clearing and cleanup.
Their Status lines record the lead's acceptance, independently of the recipes.
The release page scans driver/override strings with a debug positive control and runs a socket-free release window.
The ignored live test requires fresh human consent; it is not a workspace-test live invocation.
A fake actor test constructs authentication_failed followed by is_error true and a cancelled lifecycle while the process waits for another prompt.
It checks retirement before queued work; the account-based picker fallback is not implemented.
Live CLI behavior and performance need their authorized lanes; fake replay and structural checks cannot establish them.
Permission/approval behavior, native keyboard/IME/pointer delivery and Open Terminal's OS side effect are not proved by fake replay or the debug driver.
Stop and Interrupt have no window path, so their live continuation is unproved.
