# Veyra architecture

## Executive summary

Veyra is one native macOS chat window backed by an orchestration actor and Claude Code's stream-json process.
SQLite rows are the source of truth for projects, threads, turns, transcript items and the default permission mode.
The engine actor is the sole database writer after startup.
The gpui `Shell` owns window state and a read-only event projection, not another database connection.
The engine never depends on gpui.

Claude sessions, readiness probes, replay fixtures and debug recording are implemented.
Codex and Grok have placeholder readiness but no codec.
Claude approvals and questions reach a window dock whose actions send typed decisions through the engine.
Exact thread grants and deny/cancel replies are persisted before their provider controls run, except on storage-blocked and blocked-shutdown paths where cancellation precedes stored expiry.
Startup reaps marked orphan CLI groups, and the actor owns keep-awake, commit counts and actor timing.
The window owns render and event-batch counters.
Inactive-window family changes post system banners, and busy Quit opens a sheet.
There is no PTY or tool server.

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
Engine startup -> validated orphan sweep -> veyra-reaper thread
Engine actor -> owned caffeinate idle-sleep assertion

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

`Engine::start` locks the folder, opens `veyra.db` through a private 0600 file, migrates, reaps stored orphan groups, reconciles stored work, then starts the actor on a dedicated `std::thread`.
The actor keeps the lock file alive.
The login-shell PATH read runs synchronously before the window in its own process group.
`platform::login_shell_path` uses a private bounded helper whose two-second deadline covers shell exit and stdout EOF.
Cleanup kills only the shell's group; an escaped descendant keeping the pipe open cannot extend the deadline.
`ChildEnv` logs the error and falls back to inherited PATH.
`veyra.lock` is mode 0600 and its exclusive-lock retry lasts four seconds.
Only the lock holder stamps its pid or removes a stale debug socket.
A second launch refuses the folder and exits 1.
The OS releases the lock on exit or crash.
`process::start` validates stored pid/group pairs against a provider leader's argv and its dead-parent `VEYRA_PARENT_PID` marker before signalling.
A background sweep lists argv first and reads environments only for Claude, Codex, Grok or Veyra leaders.
The sweep covers every same-user marked agent or Veyra group whose marker parent is dead, from any data folder.
Every launch runs it, including disposable verification instances and tests that call Engine::start.
It sends SIGTERM to validated orphans and revalidates them before SIGKILL one second later.
Live parents, missing or invalid markers, reused non-agent pids and unrelated processes are left alone.
Process arguments and environment values are never logged; the macOS environment read uses `KERN_PROCARGS2`.

`main` keeps a multi-thread tokio runtime alive across the gpui loop.
The actor enters that runtime on its own thread; the UI never enters it.
The UI awaits runtime-agnostic tokio channels in gpui tasks and uses gpui timers.
Readiness work is explicitly spawned on the runtime through its handle.

After engine startup, `main` opens `logs/veyra.log` privately and installs INFO tracing.
It does not read `RUST_LOG`, so that variable cannot suppress readiness.
The debug listener binds before the window opens.
`Window::on_next_frame` prints `veyra ready pid=PID data_dir=DIR` to stderr and the log after the first frame.
`build.rs` embeds Git HEAD, or `unknown` outside Git, and watches HEAD, its reflog, packed refs and the current loose ref.
Quit opens a sheet listing running, Starting and background-busy threads, or runs ConfirmQuit immediately when none are busy.
CancelQuit dismisses it and restores composer focus.
ConfirmQuit cancels the branch task, awaits Engine::shutdown_wait in a gpui task, then calls cx.quit.
The debug ctl quit verb dispatches ConfirmQuit directly.
`Engine::shutdown_wait` refuses new commands, ends running turns, expires pending requests and asks every session to shut down.
Shutdown persists Restarted holds for running, Starting and background-busy threads in its own transaction.
It expires pending requests with "Veyra is closing" before ending turns.
A Restarted hold remains Held even without a queued turn; Send with an empty queue or Continue clears it.
Sessions queue deny/cancel replies before the protocol interrupt and give the writer up to one second to drain before group cleanup.
Cleanup terminates their owned groups and kills survivors one second later.
After the shutdown commit attempt, a three-second deadline ends the actor's session wait when its message is handled.
If that write fails, pending row changes remain blocked while sessions still receive Shutdown.
Database work and queued messages can delay handling that deadline; shutdown_wait bounds its own wait with a three-second runtime timeout.
`Engine::on_app_quit` sends SIGTERM to every registered provider/probe group, enqueues shutdown and waits 100 ms on the runtime.
The application registers on_app_quit with gpui, covering native termination as well as ConfirmQuit.
macOS termination does not promise Rust destructors, so verification explicitly cleans run-owned provider groups.

## Actor decisions, storage and recovery

`Actor` owns the SQLite connection, projection rows, live-thread map, item-key map, text buffers and per-thread event counters.
It receives typed messages through a 256-slot tokio channel.
A timer sends Tick every five seconds; text uses separate flush and write messages.
The actor never waits for provider I/O.
`actor_no_blocking_io` scans actor.rs, store.rs and commands.rs for direct filesystem, process-spawn and thread-sleep calls; indirect platform calls such as keep-awake are outside that textual guard.

`commands::decide` reads rows and runtime state, then produces a `ChangeSet` of row changes, events and effects.
Projection indexes identify each thread's turn and item ids; command overlays read those keyed rows rather than scanning every stored turn or item.
The actor completes buffered text for ended turns and finalizes newly settled task summaries before opening one transaction.
It calls `store::apply`, commits, installs the changed rows/runtime, publishes numbered events, then dispatches effects.
`ChangeSet::stores_nothing` owns an exhaustive check of its row and live-state fields; `Actor::write` skips the transaction when it returns true.
It still installs text buffers and publishes events/effects, so pure streaming deltas do not require a SQLite write.
Row-bearing events resolve from the final changed rows rather than being patched individually.
StartTurn effects name the turn until install builds its text from committed items and dispatches its control.

The send-to-reply path is ordered as follows.

1. CreateThread stores the thread and user item with a queued turn.
2. The Spawn effect starts the session on its own process group.
While the thread is still Starting, an exit before Ready fails the oldest queued turn and holds the thread as Stopped.
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
A provider, Tick or text write failure retains changes, interrupts active turns with the database-blocked reason, refuses commands and publishes StorageBlocked.
Starting sessions receive Shutdown, and idle structured sessions receive no turn interrupt or ladder.
Tick retries before draining deferred process events.
Full and closed command queues are distinct errors; Busy carries a typed WaitReason and a closed request is distinct from either.
The UI shows command and acknowledgement failures as notices.

`LiveThread` owns an epoch outside its runtime variants, capabilities and the background-work flag.
Starting stores its session handle before the actor handles any event from that session.
The engine derives thread status with `domain::thread_status` and publishes `ThreadStatusChanged` after changed thread, turn, request or live rows.
The window, sidebar, header, dump and ctl wait consume that status, including background work, NeedsYou and InTerminal.
Spawn increments the epoch; stale session events are ignored.
Turn events must also match the running turn, unless uncorrelated content starts or joins a Provider-origin turn.
An admitted `TurnEnded { park: true }` first sends Shutdown and enters `Stopping { Park }`, even if its turn already ended.
Stopping drops that session's later turn events; its Exited still matches the epoch.
The actor waits for Exited before parking and spawning eligible queued work.
A late end cannot settle another turn.
Idle structured sessions park after ten minutes without a running turn, pending request or background work.
The engine supports holds, queue reasons, cascaded Stop/Archive, Interrupt, Respond, wake delivery and task settlement.
The composer's Stop button, Ctrl+C and Send with an empty draft during a running turn dispatch the Stop action, which sends the engine's Interrupt command.
The sidebar's StopThread action sends the engine's StopThread command.
Respond, Continue and DiscardQueued also have window controls.
Archive has no window control, and task cards remain unimplemented.
On a Starting thread, Stop, Archive and Interrupt send Shutdown and wait in Stopping for Exited.
The session receives that control before stdout; if the child has not launched yet it stops the group once launch returns and emits no Ready.
A Ready from an already-running initialization is ignored after the actor leaves Starting.
On a running Structured thread, those controls end the engine turn with Interrupted and "Stopped by you", keep its session and send the protocol interrupt.
`Live::Structured.draining` keeps that ended turn's id until its own CLI end arrives.
Queued work with no hold waits with `WaitReason::Stopping` during the drain.
Send keeps an existing hold only while a turn is queued.
With no queued turn it clears the hold, so a Send after Stop waits with WaitReason::Stopping for the drain.
Continue clears a hold without adding a turn.
An uncorrelated tail from a drained Provider-origin turn is logged and dropped before it can open another turn.
The old CLI end clears the drain; the same change applies the start rule, which still refuses Stopping sessions.
Stop and Archive also discard queued user and task turns and set a Stopped hold, while Interrupt keeps them and sets an Interrupted hold.
All three preserve an existing hold.
Tasks cannot arise through the application because the tool server is not implemented.
`can_use_tool` opens a typed approval or AskUserQuestion request.
Unknown subtypes and malformed requests receive an error response without opening a request.
Malformed includes empty tool names or ids, ids reused within the process lifetime, non-object input, missing question text or option labels, and duplicate question texts.
A pending request makes the thread NeedsYou and docks its exact command or question above the composer.
AllowOnce, AllowInThread and Deny answer approvals; AnswerQuestion collects one answer per question.
Only an input whose keys are command, description or timeout may offer an exact-command thread grant.
Inputs with any other field always dock without a grant offer.
The dock shows 1 of N for multiple pending requests.
Approval chords and number keys answer only while the dock has focus; the composer keeps Command+Return as Send.
The focused dock has an accent border.
Stop expires pending requests, and the provider queues cancel replies before interrupt.
There is no automatic request timeout.
Respond persists an answer before dispatching the native reply; question answers also add a user transcript item.
The command and codec boundaries share RequestKind::accepts, whose accepted decision carries the matching grant or question/answer rows.
A mismatched codec answer sends nothing and leaves its request pending.
The codec keeps used native request ids for the process lifetime so delayed replies cannot authorize a later request.
AllowInThread stores the exact thread, tool and command in grants, and a later identical request is allowed without RequestOpened.
Other commands, tools and threads still dock, including compound commands and requests in Full access.
Turn completion and process exit expire pending requests and publish RequestClosed in the same transaction.

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
Startup validates stored live pid/group pairs before reaping; reconciliation itself does not blindly signal stored ids.

Lineage CHECKs and the parent/root foreign key refuse grandchildren and inconsistent roots.
The hold CHECK allows only the typed hold reasons or null.
The singleton settings row starts in Full access and constrains the three modes.
Thread configuration remains in `config_json`; a default-mode change never rewrites existing threads.

`reconcile::decide` runs once before commands are accepted.
The actor routes that change through `prepare_commit`, completing interrupted text and unfinished tools in the same startup transaction.
It interrupts stored running turns, expires requests and holds affected queues as Restarted without replacing an earlier hold.
It replaces an already-started unaccepted wake, preserves an unstarted wake, and leaves running child tasks open.
Terminal view returns to Chat with import_pending.
InTerminal and Terminal view are reachable only through engine tests until a PTY exists.
After startup reaping handles stored process pairs, reconciliation writes every thread Parked and clears both live_pid and live_pgid.
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

ChildEnv loads share one application-owned group registry, so the quit hook can also signal readiness probes created outside the engine.
One session task owns the child, its stdin writer and a process-group guard.
`ByteLines` caps each stdout and stderr line at 4 MiB and survives cancelled reads.
Oversized lines and invalid JSON are skipped; diagnostics reveal lengths rather than raw content.
Each accepted stdout line is parsed once and its value passes through recording, root decoding and subagent decoding.
The bounded stderr tail is redacted before display and supplies failures when the CLI exits unexpectedly.
Interrupt and Shutdown queue deny replies for every open approval and question with `interrupt: true` before the protocol interrupt.
The codec owns an active turn's interrupt ladder and both deadlines: group SIGTERM after three seconds, then SIGKILL one second later.
Typed StopReason values distinguish user, storage-blocked and app-closing interruptions.
An ended turn ignores a late interrupt, and both a turn end and a new prompt clear the old ladder.
Human interrupts use "Stopped by you"; storage and shutdown carry their own reasons.
A session without a turn never arms that ladder.
Shutdown gives queued replies up to one second to drain, then aborts an unfinished writer before group cleanup.
Blocked or failed stdin can prevent delivery; cleanup and leader reaping precede Exited.
Dropping a live group guard immediately kills its owned group and schedules cleanup.
A group still present after the final cleanup deadline produces a bounded warning with its id.
The normal session path discards other cleanup errors.
A supervisor waits for registered cleanup even if the session task panics.
The actor stores pid and group on Ready and parks on Exited.
After Stop, Archive or Interrupt it only parks.

`claude::Codec` owns stream-json envelopes, prompt correlation, native block keys, request references and background-work replacement.
Unknown protocol metadata logs its category without frame content, and unsupported or malformed control requests get an error reply.
Spawns before Ready binds a native id use --session-id with the Veyra UUID; spawns with that bound native id use --resume.
Mode maps to default, acceptEdits or bypassPermissions plus --allow-dangerously-skip-permissions.
An initialize reply can open the Ready gate before the prompt causes system/init to supply native id, version and capabilities.
Prompt UUID echoes and lifecycle acknowledgements separate Veyra prompts from foreign results and Provider-origin work.
Uncorrelated CLI work may open a Provider turn after the foreground turn completes.
Assistant error words latch per prompt, and account-related classes park the process.
An active Veyra interrupt overrides that error outcome and suppresses newly arriving diagnostic text, including streamed blocks.
Without an active Veyra interruption or latched assistant error, externally observed SIGHUP, SIGINT, SIGTERM or conventional signal exit codes produce Interrupted.
Cleanup-generated signals do not change the outcome.
After stdout EOF, the session polls try_wait during the existing 500 ms stderr drain to observe the natural leader status before signalling cleanup.
It publishes a synthesized turn end before awaiting group cleanup, so post-KILL waits cannot extend the interrupt boundary beyond five seconds.
Exited still follows cleanup, and the actor starts no queued work before Exited.
Other abnormal exits produce Failed with Veyra's bounded, redacted crash text.
Streamed and final blocks share native keys so final frames do not duplicate delivered text.
Each background_tasks_changed replaces the pending set; ambient tasks never keep the thread Working.
Child frames feed the pending root Task or Agent tool's bounded output tail instead of creating root text or turns.
Unknown or finished child parents are ignored, and snapshots retain a running tool's tail.
Expanded tool rows show the live tail while running and prefer the result tail after completion.
Compact, reset, retry, usage-limit and model-fallback frames add notices in Veyra's words.
Only a resume-not-found stderr class starts a fresh native session; other errors retain the native id for Retry.

## Readiness, fixtures and recording

Startup, Check again and Retry probe each provider independently on the runtime.
`Probes` owns each provider's cancellable task.
A replacement cancels and waits for the earlier task, including its process-group ladder, before starting.
Shell generation numbers reject stale probe replies.
There is no probe timeout; a hung Claude stays Checking while the other providers become Ready.
Claude resolves its path, reads its version, launches stream-json in the private data folder with no session flag, sends initialize and matches the request_id without a turn.
It reads models and optional account, then ends and waits for the probe process through the same group ladder.
A missing binary is Not installed.
Except for a missing binary or the captured signed-out signal, every probe failure or inconclusive reply reads Ready.
An account with apiProvider and tokenSource but no signed-in identity fields reads Signed out, following Veyra's recorded Claude signal.
Missing or empty account remains unknown and Ready.
Both version and initialize reads reuse capped byte lines, and stderr is drained so a chatty probe cannot block its stdout reply.
Codex and Grok are always Ready placeholders and run no probe command.
Their Send is enabled, but spawn fails with "this provider is not built yet", fails the oldest queued turn and holds the Starting thread as Stopped.
The Shell holds raw probe account metadata in memory.
Dump exposes only account presence as known or unknown, never its value.

`fixture.rs` defines Veyra's transcript_start, expect_outbound, emit_inbound, emit_raw, touch, spin and exit records.
Touch creates a file under the fake CLI's cwd and rejects absolute paths and parent components.
Spin burns CPU for its supplied milliseconds.
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
Entering Settings clears the displayed branch and lets an in-flight lookup finish; its selection and view checks discard that late reply.
Other view changes and Quit call `forget_branch` to cancel the task and clear the displayed branch.
Dropping the Shell also cancels the task, and its reply checks the selection identity.
The centred transcript uses a window-owned MessageScrollerState over gpui ListState and selectable Markdown TextView.
The transcript and its expansion behavior live in ui/transcript.rs.
Failed tools show their last nonempty output line, truncated to one row at its width.
The window and dump share row_outcome, which annotates only each ended turn's final item.
A user scroll detaches following; new text, requests and tool rows preserve that state.
Scrolling within text.read.line_height of the bottom or choosing Latest reattaches.
Latest appears only while following is detached and content remains below.
Expanding a row remeasures it without moving its top anchor.
The actor trims stored tool output to its last 10,000 characters.
The actor publishes window thread wait reasons and busy flags after reconciliation, then updates only threads whose thread, turn, request or live rows changed.
Engine::window_threads reads that snapshot.
The live::busy predicate is shared by quit-sheet publication, shutdown holds and keep-awake.
Streamed text and tool input remain unbounded outside the 4 MiB wire-line cap.
The failed view uses the latest nonqueued turn and shows the cause, next step, Retry and Open Terminal on the project folder.
Retry probes again; once the thread's provider is Ready it clears any hold and resends the failed turn's own user item as a new turn.
A newer queued message remains queued and is never reused as the Retry prompt.
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

`ui/actions.rs` owns actions, the native menu and one bindings table used by both key installation and shortcut_chip.
Ctrl+C is literal control in the composer and preserves its draft while interrupting the turn.
Approval chords are Command+Return, Command+Option+Return and Command+period while the dock has focus.
NextNeedsYou scans sidebar order for NeedsYou before Failed, wrapping once and including child rows.
The composer shows Stop while a turn runs and its draft is empty, plus held reasons and Continue.
Discard queued appears only when a held thread has queued turns.
The quit sheet overlays the existing window content.
The Theme global owns all colours, geometry, type, radii and motion tokens.
Theme::apply projects them through the kit and follows macOS appearance.
Theme values and roles live in ui/theme.rs.
Geist and Geist Mono are bundled from Vercel's official release with its OFL beside them.
Window geometry comes from the theme and can be overridden by the window manager.

`ui/notify.rs` derives banners only when an inactive window's family enters NeedsYou, Done or Failed.
Family status includes unarchived children, so pending background work prevents a Done banner.
The banner contains the family root title, derived by collapsing whitespace in its first user message and taking the first 80 characters, and a status word.
It excludes assistant, tool and later user text.
Its root-id tag lets a click activate the window and select the root or the first child needing attention.
`scripts/bundle-app.sh` builds an unsigned macOS app bundle with dev.veyra.app identity for system notification delivery.
System permission and native banner clicks require operator verification.
The debug-only DebugNotificationResponse action passes a banner tag to the same notification_response handler as the native callback, allowing stale-tag routing checks without OS delivery.

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

Action accepts optional JSON data, keys parses before dispatch, text uses the focused textarea's public input handler, and quit dispatches ConfirmQuit.
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
The platform module owns macOS file modes, locks, sockets, login-shell PATH, executable checks, group setup/signals, process inspection, idle-sleep assertions and Terminal opening.
Each agent and probe spawn attempts to set group nice 10 and logs failure while continuing.
The actor attempts to hold one caffeinate guard while any thread is Starting, has a running turn or has non-ambient background work.
An assertion-helper spawn failure is logged without stopping work.
The pure needs_awake predicate controls that guard, whose drop stops its owned caffeinate child.
Dump's awake reports whether the actor retains the guard, without checking the child or OS assertion after spawn.
Dump also reports each known live CLI pid with resident_bytes.
Debug perf reports commits, actor_max_ms, actor_busy_ms, repaints and max_batch_events.
actor_busy_ms is the elapsed time of the message currently being handled, or zero when idle.
Dump includes every request row the window has received, including its state, plus quit_sheet, window_active, notifications.posted and transcript.items with transcript.follow.
Every actor message is timed, and a message over 50 ms logs its kind and duration.
DebugStallUi blocks the window thread for its supplied milliseconds so the pump can be measured under a controlled stall.
Platform module selection rejects unsupported targets with compile_error.
The textual platform_boundary test is a guard, not proof against every indirect OS call; review still checks indirect commands and strings.

## Source map

- [Entry and runtime](crates/veyra/src/main.rs), [engine handle](crates/engine/src/lib.rs), [data folder](crates/engine/src/data_dir.rs).
- [Actor](crates/engine/src/actor.rs), [decisions](crates/engine/src/commands.rs), [domain/indexes](crates/engine/src/domain.rs), [live variants](crates/engine/src/live.rs).
- [Storage/schema](crates/engine/src/store.rs), [recovery](crates/engine/src/reconcile.rs), [events/snapshots](crates/engine/src/events.rs).
- [Provider boundary](crates/engine/src/provider/mod.rs), [environment](crates/engine/src/provider/env.rs), [sessions/probes](crates/engine/src/provider/session.rs), [Claude codec](crates/engine/src/provider/claude.rs).
- [Fixture format](crates/engine/src/fixture.rs), [fake CLI](crates/engine/src/bin/veyra-fake-cli.rs), [recording/redaction](crates/engine/src/record.rs).
- [Window/pump/fence/branch](crates/veyra/src/ui/mod.rs), [transcript](crates/veyra/src/ui/transcript.rs), [composer](crates/veyra/src/ui/composer.rs), [Settings](crates/veyra/src/ui/settings.rs), [actions](crates/veyra/src/ui/actions.rs), [theme](crates/veyra/src/ui/theme.rs).
- [Banner decisions](crates/veyra/src/ui/notify.rs), [macOS app bundle](scripts/bundle-app.sh).
- [Process supervision](crates/engine/src/process.rs), [Platform mechanisms](crates/engine/src/platform/macos.rs), [selection](crates/engine/src/platform/mod.rs), [boundary guard](crates/engine/src/platform/boundary.rs).
- [Driver](crates/veyra/src/control.rs), [name coverage](crates/veyra/src/verify_map.rs), [verification skill](.agents/skills/verify-veyra/SKILL.md).

## Verification

Workspace tests cover private paths and modes, launch locking, platform_boundary, command-table behavior, recovery, storage, sessions and theme contrast.
replay_claude uses the real session task with the fake CLI for text and tools, prompt correlation, provider-turn completion, resume, crash and error outcomes, background replacement and readiness.
It covers approvals, questions, unknown requests, Stop with a pending approval and late interrupts.
It also checks assistant-error parking, interrupt outcomes and ladder cancellation, subagent isolation against the live Agent recording, notices and malformed-byte recovery.
Actor tests cover retirement before queued work, the Stop drain, Provider-tail rejection and skipped text-only transactions.
Platform tests check the two-second login-shell bound, inherited-pipe deadline and group nice value.
Session tests check process exit classification and cleanup ordering.
Request, grant and awake tests check request expiry, exact matching, thread isolation, needs_awake and retained-guard flags.
They do not inspect caffeinate liveness or the OS assertion lifetime.
Reaper tests check live parents, non-agent argv and the environment-read boundary.
record_redaction covers token removal, private captures, version/account sanitization and a fixture secret-shape sweep.
Tests guard failure redaction, launch arguments, terminal outcomes, snapshot ordering, metadata delivery, wait exits and driver-name coverage.
Application tests enable gpui test-support to exercise Shell selection and its event pump in a synthetic window.
Shell tests load the login-shell PATH as the app does and run the startup orphan sweep.
Their current-thread runtimes do not drive readiness tasks before replacing the probes, so those tests start no provider check.
The native-quit test is also verified with an executable stand-in whose access time remains unchanged.
Clippy with -D warnings and cargo fmt --check check source quality.
The verify-veyra pages cover the real window, launch and relaunch row preservation, readiness without threads, streaming and background status, failure and Retry, default-mode persistence, header-branch clearing and cleanup.
Approvals-questions, stop-interrupt and restart-recovery cover the dock, queue controls, banners, keep-awake, quit sheet and startup recovery.
Permission-matrix records separately authorized live permission checks.
Their Status lines record the lead's acceptance, independently of the recipes.
The release page scans driver/override strings with a debug positive control and runs a socket-free release window.
The ignored live test requires fresh human consent; it is not a workspace-test live invocation.
Actor tests check account-error retirement before queued work; a separate probe fixture checks Signed out while the no-account fixture remains Ready.
Live CLI behavior and performance need their authorized lanes; fake replay and structural checks cannot establish them.
Real provider permission behavior, native keyboard/IME/pointer delivery and Open Terminal's OS side effect are not proved by fake replay or the debug driver.
Composer Stop sends Interrupt and sidebar StopThread sends StopThread; real-provider continuation requires its authorized live lane.
