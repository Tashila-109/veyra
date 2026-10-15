# Veyra architecture

## Executive summary

Veyra is one native macOS chat window backed by an orchestration actor and per-thread Claude stream-json or Codex app-server processes.
SQLite rows are the source of truth for projects, threads, turns, transcript items, tasks and the default permission mode.
The engine actor is the sole database writer after startup.
The gpui `Shell` owns window state and a read-only event projection, not another database connection.
The engine never depends on gpui.

Claude and Codex sessions, readiness probes, replay fixtures and debug recording are implemented.
Grok has placeholder readiness and no codec.
Claude and Codex server approvals and questions reach the same window dock, whose actions send typed decisions through the engine.
Codex async agent-message questions instead become persistent transcript cards whose option actions send ordinary user messages.
Exact thread grants and deny/cancel replies are persisted before their provider controls run, except on storage-blocked and blocked-shutdown paths where cancellation precedes stored expiry.
Startup reaps marked orphan CLI groups, and the actor owns keep-awake, commit counts and actor timing.
The window owns render and event-batch counters.
Inactive-window family changes post system banners, and busy Quit opens a sheet.
There is no PTY.
Engine startup binds the MCP socket server, and root Claude and Codex sessions receive its five actor-owned tools through the relay.

### System architecture

```text
User -> macOS window -> Shell + Theme + textarea + MessageScroller
                            | commands / acknowledgements
                            | subscription / fenced snapshots
main -> tokio runtime -> Engine actor -> veyra.db
                              | spawn / controls
                              v
                         provider session -> owned CLI process group
                              | normalized events
                              +--------------------> Engine actor
Shell -> runtime readiness probes -> initialize-only CLI process
Shell -> runtime branch task -> git child
Engine startup -> validated orphan sweep -> veyra-reaper thread
Engine actor -> owned caffeinate idle-sleep assertion

root CLI -> veyra mcp direct child -> DATA/veyra.sock -> peer + token checks -> Engine actor
Claude session -> private run/THREAD/mcp.json -> Claude root CLI
Codex session -> thread/start or thread/resume config -> Codex root CLI

debug ctl -> private Unix socket -> socket thread -> gpui window actions
                                         debug session -> recorder / fake CLI
```

### Dependency hierarchy

```text
veyra -> veyra-engine -> rusqlite (bundled) + tokio
                     -> serde + serde_json + uuid + thiserror + tracing + regex + base64 + sha2
                     -> platform -> libc + Rust standard library
veyra -> gpui-kit + gpui-pre
      -> tokio + serde + serde_json + tracing + tracing-subscriber + anyhow
```

Direct versions are exact pins and `Cargo.lock` is kept.
The engine's tokio features are `sync`, `rt-multi-thread`, `time`, `macros`, `process`, `io-util` and `net`; engine tests add `test-util`.
Only the application crate declares gpui dependencies.
`platform_boundary` scans for libc, socket, process-group, target and listed command strings outside `platform/`; the header's `git` child is a reviewed exception.
Manifest review keeps gpui out of the engine and direct versions exact; no automated manifest check enforces those rules.

## Startup and data ownership

`main::run` handles arguments before creating folders or starting gpui.
No argument opens the window; debug builds route `ctl` to its client.
The `mcp` argument runs the byte relay before data resolution, locking, logging, the engine or gpui.
Other arguments print `usage: veyra [mcp]` and exit 2.

`DataDir::resolve` reads `VEYRA_DATA_DIR`, or uses `~/Library/Application Support/Veyra`.
Relative overrides become absolute paths.
It validates the longest socket path before creating the data folder, `logs/` and `run/` at mode 0700.
The platform limit is 103 usable socket-path bytes.
Debug builds validate the longer `control.sock` path; release builds validate `veyra.sock`.
File-backed Engine startup binds veyra.sock in both builds; command-path, private-folder, stale-config removal and bind failures refuse startup.
Before binding, mcp::remove_stale_configs removes stale mcp.json files in UUID-named run folders, including configs left by a crash; a sweep error refuses startup.

`Engine::start` loads the production provider environment, then locks the folder, opens `veyra.db` through a private 0600 file, migrates, reaps stored orphan groups, reconciles stored work and consumes the storage recovery receipt, removes stale run configs, binds veyra.sock, then starts the actor on a dedicated `std::thread`.
The actor keeps the lock file alive.
`Engine::start_with_env` accepts a prepared environment for isolated tests; the handle retains its resolution policy without owning another MCP server reference.
Ordinary unit-test `ChildEnv::load` calls and explicit isolated integration/window test environments never search for installed provider binaries.
`ChildEnv::isolated` starts with no provider paths, and `with_binary` adds only an explicit fake or missing path in either build profile.
Ignored live tests use the production loader.
The production loader keeps its login-shell PATH, debug overrides and unrestricted release reaper behavior.
The login-shell PATH read runs synchronously before the window in its own process group.
`platform::login_shell_path` uses a private bounded helper whose two-second deadline covers shell exit and stdout EOF.
Cleanup kills only the shell's group; an escaped descendant keeping the pipe open cannot extend the deadline.
`ChildEnv` logs the error and falls back to inherited PATH.
`veyra.lock` is mode 0600 and its exclusive-lock retry lasts four seconds.
Only the lock holder stamps its pid or removes a stale debug socket.
A second launch refuses the folder and exits 1.
The OS releases the lock on exit or crash.
`process::start` resolves canonical provider binaries through the engine's ChildEnv before validating stored pid/group pairs.
A provider leader is recognized by a provider argv basename, by node, bun or a shell running a provider script, or by a canonical executable path matching one of those binaries.
Its dead-parent `VEYRA_PARENT_PID` marker must also validate before signalling.
A background sweep lists argv, checks the group-leader rule once, and lazily reads executable paths before reading environments only for recognized provider or Veyra leaders.
The macOS executable lookup uses `proc_pidpath`, so versioned native-installer binaries reached through provider symlinks qualify.
Executable matching can include every leader of a shared shim that a provider name resolves to; the dead-parent marker check still gates every signal.
Provider leaders started by session hooks or tools also qualify when they inherit that marker and its parent is dead.
The sweep covers every same-user marked agent or Veyra group whose marker parent is dead, from any data folder.
Normal launches run the unrestricted sweep.
Reaper tests pass a process-list closure filtered to their own child pids; synthetic Shell tests inject an isolated environment, and live approval tests use Engine::start_with_processes with an empty owned set.
The debug-only VEYRA_REAP_PIDS override restricts verification startup to saved run-owned provider pids; an empty list permits no signals.
It sends SIGTERM to validated orphans and revalidates them before SIGKILL one second later.
Live parents, missing or invalid markers, reused non-agent pids and unrelated processes are left alone.
Each successful orphan-group signal writes an INFO line with pid, group and reason, without arguments or environment values.
Process arguments and environment values are never logged; process-list lookup uses proc_listallpids and getpgid, and the macOS argument and environment reader uses `KERN_PROCARGS2`.
The PID snapshot reserves 64 spare entries between its size and fill calls, an implementation choice for a short creation burst costing 256 extra bytes.
ProcessBlock separates argv from the environment bytes, preserving argument boundaries; enumeration decodes only argv and discards the environment portion.
Environment lookup examines that portion only for a named key after the reaper's recognition checks.

`main` keeps a multi-thread tokio runtime alive across the gpui loop.
The actor enters that runtime on its own thread; the UI never enters it.
The UI awaits runtime-agnostic tokio channels in gpui tasks and uses gpui timers.
Readiness work is explicitly spawned on the runtime through its handle.

Before engine startup, `main` opens `logs/veyra.log` privately and installs INFO tracing so startup reap signals are recorded.
It does not read `RUST_LOG`, so that variable cannot suppress readiness.
The debug listener binds before the window opens.
`Window::on_next_frame` prints `veyra ready pid=PID data_dir=DIR` to stderr and the log after the first frame.
`build.rs` embeds Git HEAD, or `unknown` outside Git, and watches HEAD, its reflog, packed refs and the current loose ref.

## Quit and shutdown

Quit opens a sheet listing running, pending- or refused-admission, Starting and background-busy threads, or runs ConfirmQuit immediately when none are busy.
CancelQuit dismisses it and restores composer focus until ConfirmQuit begins; afterward Escape and CancelQuit keep the sheet open.
ConfirmQuit synchronously closes the engine and cancels readiness probes and the branch task, then awaits Engine::shutdown_wait in a gpui task and calls cx.quit.
The debug ctl quit verb dispatches ConfirmQuit directly.
`Engine::shutdown_wait` refuses new commands, ends running and pending-admission turns, expires pending requests and asks every session to shut down.
Shutdown persists Restarted holds for running, queued, Starting and background-busy threads in its own transaction, without replacing an earlier hold.
The engine owns one LaunchGate shared with the actor, every CLI session, ChildEnv and Probes.
It combines an atomic closing state with a small synchronous lock; no guard crosses an await.
Engine::shutdown sets closing while holding the gate before enqueueing Shutdown, asynchronously sending that message if the mailbox is full.
Dropping the last Engine handle and Actor::shutdown also set closing under the gate.
The Shell reads Engine::is_closing rather than keeping its own flag, and begin_quitting closes the engine and cancels probes and branch work for both quit paths.
The actor never holds the launch gate across decisions, SQLite writes, installation or effect dispatch, so a synchronous Spawner::spawn or Spawner::terminal can enter the launch boundary without re-entering a held lock.
Quit can wait for multiple contending launch critical sections at the gate, never for the actor or SQLite.
Queued turns other than a pending admission remain Queued and held, retiring sessions become Parked without a replacement, and Starting threads without a session handle park during shutdown rather than waiting for an impossible exit.
If the shutdown write fails, a Starting thread that still owns a session can instead fail its oldest queued turn when that session exits.
A posted spawn error for a sessionless Starting thread meets its Parked runtime state and keeps the queued turn held.
Actor::prepare_commit debug-asserts that no start or spawn effect leaves a shutdown decision.
It expires pending requests with "Veyra is closing" before ending turns.
A root's Restarted hold remains Held even without a queued turn; a recovered child without queued work shows its latest turn's outcome, Interrupted after recovery of a running turn, and retains Continue.
Send with an empty queue or Continue clears the hold.
Sessions queue deny/cancel replies before the protocol interrupt and give the writer up to one second to drain before group cleanup.
Cleanup terminates their owned groups and kills survivors one second later.
Shutdown first acknowledges any streamed buffer already materialized in its retained changes, installs their projection without publishing or dispatching their earlier effects, then combines them with shutdown changes for one write.
Acknowledgement moves text ownership into those retained item rows even while their write remains blocked, so shutdown finalization appends it only once.
Persistent actors retry a blocked shutdown write every 50 ms within the existing three-second deadline; this is Veyra's implementation choice to give a released database lock another chance without a busy loop.
In-memory actors still end promptly when no session remains.
After the shutdown commit attempt, a three-second deadline ends the actor's session wait when its message is handled.
If that write fails, pending row changes remain blocked while sessions still receive Shutdown, and sessionless Starting threads still park in runtime state.
A blocked write logs affected thread ids and changed item, turn, request, task and metadata identifiers without recording their text.
A private DATA/run/storage-blocked.json receipt carries that same metadata for the next launch and is removed after a successful write.
Startup chooses storage-loss notice targets from the reconciled turn rows, including attempted starts recovered as Interrupted, and stores those notices in the startup transaction before clearing the valid receipt.
A receipt entry without a started turn after reconciliation cannot attach a transcript notice.
An unreadable or malformed receipt is renamed aside privately when possible, logged without its contents and reported through a window recovery notice; it does not refuse startup.
The notice identifies what may have been lost and asks the user to inspect the provider conversation before sending more work.
Receipt creation failures are logged; unavailable storage can prevent the next-launch notice from being saved.
Database work and queued messages can delay handling that deadline; shutdown_wait bounds its own wait with a three-second runtime timeout.
`Engine::on_app_quit` closes the engine before sending SIGTERM to every registered provider/probe group, then waits 100 ms on the runtime.
The application registers on_app_quit with gpui and runs begin_quitting before awaiting native termination cleanup.
macOS termination does not promise Rust destructors, so verification explicitly cleans run-owned provider groups.

### Quit commitment points

- Actor admission: the atomic closing read at the start of a message is its commitment point.
A command or provider event past a read of open commits as pre-quit work; a read of closing refuses new work and keeps earlier queues.
Shutdown ends and holds an admitted turn for recovery, while the prompt writer refuses a prompt whose own closing check has not passed.
Storage retry uses Tick's admission read before writing and installing retained changes, and each deferred message re-enters admission.
Actor::exited reads the closing state directly before session and terminal restarts or queued-turn starts.
Effects run with no launch gate held and refuse starts while closing.
- Launch with registration: ChildEnv::spawn holds the gate through its closing check, OS spawn and group registration, so quit either refuses the launch or sees its group in the signal pass.
- Prompt write: the stdin writer uses an Acquire read of the atomic closing state immediately before each user frame and drops that frame if closing won, while preserving cancel replies before interrupt.
The prompt check is its commitment point; a write already past it belongs to a turn admitted before quit and shutdown interrupts that turn by protocol.
The writer takes no gate and an already committed write can finish after closing.

## Actor decisions, storage and recovery

`Actor` owns the SQLite connection, projection rows, live-thread map, item-key map, accepted-turn id set, text buffers and per-thread event counters.
It receives typed messages through a 256-slot tokio channel.
Engine::send returns a future that awaits mailbox space and then the command acknowledgement.
The window awaits that future in its gpui task; tokio's FIFO permit queue preserves the order of waiting sends.
Subscription and snapshot requests still use try_send and close their reply channels when the mailbox is full.
A timer sends Tick every five seconds; text uses separate flush and write messages.
The actor never waits for provider I/O.
The actor trims stored tool output to its last 10,000 characters.
The actor publishes window thread wait reasons, busy flags and pending-admission turn ids after reconciliation, then updates only threads whose thread, turn, request or live rows changed.
ChangeEvent boxes its large ThreadCreated and RequestOpened/RequestClosed row payloads to keep UiEvent compact; the window stores their unchanged row values.
Engine::window_threads reads that snapshot, including the pending-admission turn id for window Stop targeting.
ThreadView::turn_busy includes a running row or any admission state, Pending or Refused; live::busy adds Starting and background work, and live::busy_threads supplies the shared walk for shutdown holds and needs_awake.
Every actor message is timed, and a message over 50 ms logs its kind and duration.
`actor_no_blocking_io` scans actor.rs, store.rs and commands.rs for direct filesystem, process-spawn and thread-sleep calls; indirect platform calls such as keep-awake are outside that textual guard.

`commands::decide` reads rows and runtime state, then produces a `ChangeSet` of row changes, events and effects.
Projection indexes identify each thread's turn and item ids; command overlays read those keyed rows rather than scanning every stored turn or item.
The actor completes buffered text for ended turns and finalizes newly settled task summaries and source links before opening one transaction.
It calls `store::apply`, commits, installs the changed rows/runtime, publishes numbered events, then dispatches effects.
`ChangeSet::stores_nothing` owns an exhaustive check of its row and live-state fields; `Actor::write` skips the transaction when it returns true.
The actor still installs text buffers and publishes events/effects, so pure streaming deltas do not open a SQLite transaction.
A repeated Ready stores thread metadata only when it changed.
The live pid/group UPDATE affects only rows whose stored pair differs.
Actor::write rolls back a transaction with no affected rows rather than committing or counting it; runtime changes and events still install.
Row-bearing events resolve from the final changed rows rather than being patched individually.
StartTurn effects name the turn until install builds its text from committed items and dispatches its control.

The send-to-reply path is ordered as follows.

1. CreateThread stores the thread and user item with a queued turn.
2. The Spawn effect starts the session on its own process group.
While the thread is still Starting, an exit before Ready fails the oldest queued turn and holds the thread as Stopped.
3. Claude publishes Ready on a matched initialize reply or system/init; Codex publishes it on a successful thread/start or thread/resume reply, with the returned thread id and cliVersion, after any not-found fallback or active-writer retry.
Ready changes Starting to Structured.
4. start_changed makes the queued turn Running, records pending admission and dispatches StartTurn with committed prompt text.
The first prompt waits for Ready.
The codec admits StartTurn only with no active turn, and the session reports StartTurnRefused when CLI work is already running.
ProviderTurnStarted opens the CLI's own turn under its reported id and restores a pending Veyra turn to Queued with its original ordinal.
The queue waits with TurnRunning while that Provider turn runs, an admission remains pending, or a refused start waits for the CLI turn to end.
Pending or Refused admission counts as busy for StopThread, Archive, Configure, SwitchView, idle parking, keep-awake and Quit.
StopThread and Archive target both a running row and a pending start; Interrupt targets only its named running or pending turn on a Structured session, and the session ignores a target unless it accepted that turn.
A Refused admission names no turn, so window Stop has no target unless a Running row is also rendered.
A refusal replaces Pending admission with Refused until the next CLI turn end; the first correlated turn event confirms an admitted prompt and restores its Running row if CLI work displaced it.
The Provider end applies start_changed, which retries eligible queued work once pending admission has resolved.
The Claude codec also emits Ready on system/init, so an early system/init can open the Ready gate before the initialize reply.
5. The codec normalizes deltas; the actor buffers text and counts actual stream deltas before flushing.
6. Item and turn completion persist final rows, free streaming buffers and publish the engine-owned status.

Text coalesces per item, normally flushing to the UI at most once per 16 ms.
Requests, item completion and turn completion flush pending UI text immediately.
Unpersisted text writes every two seconds and at item/turn completion.
Completed items and ended turns free their item keys and text buffers.
Ended turns also remove their accepted-turn set entries in the same installation pass.
A command write failure returns an error without publishing the decision.
A provider, Tick or text write failure retains changes, interrupts active turns with the database-blocked reason, refuses commands and tool calls, and publishes StorageBlocked.
Every blocked write preserves the metadata receipt described in Quit and shutdown.
Storage-related tool refusals name recovery of storage and a running root turn as prerequisites, then give the exact retry call and arguments, preserving task request keys.
Starting sessions receive Shutdown; every structured session receives InterruptActive, which interrupts the codec's active turn even before its actor row exists and leaves an idle codec without a ladder.
Tick retries before draining deferred process events.
A closed actor channel returns Closed; Busy carries a typed WaitReason and a closed request is distinct from either.
The UI shows command and acknowledgement failures as notices.

`LiveThread` owns an epoch outside its runtime variants, capabilities, the background-work flag and optional TurnAdmission, either Pending with a turn id or Refused.
Starting stores its session handle before the actor handles any event from that session.
The engine derives thread status with `domain::thread_status` and publishes `ThreadStatusChanged` after changed thread, turn, request or live rows.
The window, sidebar, header, dump and ctl wait consume that status, including background work, NeedsYou and InTerminal.
Spawn increments the epoch; stale session events are ignored.
Turn events must also match the running turn, unless uncorrelated content joins the Provider-origin row opened by ProviderTurnStarted.
An admitted `TurnEnded { park: true }` first sends Shutdown and enters `Stopping { Park }`, even if its turn already ended.
Stopping drops that session's later turn events; its Exited still matches the epoch.
The actor waits for Exited before parking and spawning eligible queued work.
A late end cannot settle another turn.
Idle structured sessions park after ten minutes without a busy turn, pending request or background work.
The engine supports holds, queue reasons, cascaded StopThread/ArchiveThread, Interrupt, Respond, wake delivery and task settlement.
The composer's Stop button, Ctrl+C and Send with an empty draft during a running turn or a pending admission dispatch the Stop action, which carries the rendered thread and turn into the engine's Interrupt command.
The root render captures one StopTarget for both its keyboard handlers and the composer button.
A Stop button id names its displayed turn, so releasing an earlier mouse press after a repaint cannot stop a later turn.
The sidebar's StopThread action sends the engine's StopThread command.
Respond, Continue and DiscardQueued also have window controls.
Archive and ArchiveKeepChildren are window actions bound in the shared key table and exposed in each thread row menu.
Archive on a root stops and hides its whole family; Archive on a child stops and hides only that child and holds its parent as ChildReport unless it already has a hold.
ArchiveKeepChildren acts only on a root with an unarchived child, matching the row menu, and keeps those children running.
Task and family cards read the existing task, lineage and status projection.
On a Starting thread, StopThread and Archive send Shutdown and wait in Stopping for Exited.
Interrupt is a no-op outside Structured, including Starting because it has no running or pending turn.
The session receives that control before stdout.
A Shutdown already pending at the launch decision prevents launch and emits Exited; if it arrives during launch, the session stops the owned group once launch returns and emits no Ready.
A Ready that arrives after StopThread or Archive has moved the thread to Stopping is dropped because Stopping holds no session.
On a running Structured thread, Interrupt, StopThread and Archive end the engine turn with Interrupted and "Stopped by you", keep its session and send an Interrupt control for that turn; the codec writes the protocol interrupt only while that turn remains active.
`Live::Structured.draining` keeps that ended turn's id until its own CLI end arrives.
Queued work with no hold waits with `WaitReason::Stopping` during the drain.
Send keeps an existing hold only while a turn is queued.
With no queued turn it clears the hold, so a Send after Stop waits with WaitReason::Stopping for the drain.
Continue clears a hold without adding a turn.
An uncorrelated tail from a drained Provider-origin turn is logged and dropped.
The old CLI end clears the drain; the same change applies the start rule, which still refuses Stopping sessions.
StopThread and Archive also discard queued user and task turns and set a Stopped hold, while Interrupt keeps them and sets an Interrupted hold.
All three preserve an existing hold.
Authenticated root-session EngineMsg::Tool requests create and manage tasks through the actor.
`can_use_tool` opens a typed approval or AskUserQuestion request.
Unknown subtypes and malformed requests receive an error response without opening a request.
Malformed includes empty tool names or ids, ids reused within that CLI process lifetime, missing or non-object input, missing question text or option labels, and duplicate question texts.
A pending request makes the thread NeedsYou and docks its exact command, tool target or question above the composer.
The Claude codec turns each approval into ProposedAction, a tool label and provider-neutral ApprovalTarget: Command, File or Tool.
Command preserves exact command text; File carries its operation, literal target path, location and preview; Tool carries its argument preview.
File previews include content and edit strings, and other tool previews include every argument as pretty-printed text.
Command previews include all arguments other than the separately displayed command.
Each preview retains at most TOOL_OUTPUT_CHARS, the same 10,000-character cap as tool output, and carries a truncation flag rendered as a cut notice.
Write, Edit, MultiEdit and Read use file_path; NotebookEdit uses notebook_path.
A missing target or a conflicting file_path, path or notebook_path becomes a Tool action showing every argument without a file-location claim.
Relative targets and targets starting with ~ are NotVerified.
The codec classifies absolute location once, synchronously in the session task, when the request opens against the canonical project folder.
It canonicalizes the deepest existing ancestor and appends the missing remainder; a remainder containing a parent component or an unresolvable path is NotVerified.
InsideProject requires the resolved target to be under the canonical project folder; proved other targets are OutsideProject.
Request rows store only this typed display form, with no duplicated raw input.
Legacy stored command rows load as Command; rows with optional legacy input load as Tool previews without claiming a verified file location.
AllowOnce, AllowInThread and Deny answer approvals; AnswerQuestion collects one answer per question.
Claude offers an exact-command thread grant only for a Bash Command action whose input keys are command, description or timeout.
Claude inputs with any other field always dock without a grant offer.
Codex offers a command grant only when the request names exactly the thread's project cwd; another or unverified cwd always docks without a grant.
CommandCwd stores ProjectFolder, the other literal cwd, or NotVerified beside the exact command, and the dock uses it for the consequence text.
The grants table remains keyed by thread, tool and command.
StopThread and Interrupt expire pending requests, and the provider queues cancel replies before interrupt.
There is no automatic request timeout.
Respond persists an answer before dispatching the native reply; question answers also add a user transcript item.
The command and codec boundaries share RequestKind::accepts, whose accepted decision carries the matching grant or question/answer rows.
A mismatched codec answer sends nothing and leaves its request pending.
The codec keeps used native request ids for the life of that CLI process so delayed replies cannot authorize a later request.
A respawn starts a new codec, and exit expires the engine's pending rows.
An engine respawn uses a new session channel; the resume-not-found relaunch reuses the channel, but its fresh codec drops answers for ids it never opened.
AllowInThread stores the exact thread, tool and command in grants, and a later identical request is allowed without RequestOpened.
Other commands, tools and threads still dock, including compound commands and requests in Full access.
Turn completion and process exit expire pending requests and publish RequestClosed in the same transaction.

`store::MIGRATIONS` owns schema 3, with WAL, `synchronous=NORMAL`, foreign keys and a two-second busy timeout.
Migrations use `PRAGMA user_version` in transactions and refuse newer schemas.
The eight tables are projects, threads, turns, items, requests, grants, tasks and settings.
User text lives once in an item referenced by its turn.
An ended turn carries both its optional start time and end time, including after load.
A requeued turn retains started_at in SQLite, and store::load fills Projection.attempted_starts only for queued rows with a stored start so reconcile can see an uncertain admission.
Runtime turn insertion does not fill that recovery-only index.
Reconcile interrupts a queued turn with an earlier start attempt because it cannot know whether the CLI received the dispatched prompt.
The prompt remains in its existing user item, and Continue cannot resend that uncertain attempt.
Unstarted queued turns remain queued and held; pending wakes keep the existing safer duplicate-report recovery.
Migration 2 normalizes older empty CLI versions to NULL once.
Migration 3 adds the nullable tasks.source_item_id foreign key to items; older tasks load with no source link.
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

## Shared provider process boundary

`provider/mod.rs` exposes typed controls and normalized events to the actor, not Claude wire types.
The actor dispatches through `Spawner::spawn`.
Its `Cli` variant sends built Claude and Codex adapters to `session::spawn_provider` with the Engine's shared `ChildEnv`; Grok returns NotBuilt.
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

ChildEnv loads create independent group registries; Engine::child_env shares the engine registry with UI readiness probes, and sessions inherit it from the actor environment.
Engine::child_env gives UI probes the engine's LaunchGate, while sessions inherit it from the actor's ChildEnv.
ChildEnv::spawn checks closing, spawns and constructs the tracked ProcessGroup in one gate-held operation, reporting Cancelled without launching when closing is set.
ProcessGroup owns both registry insertion and removal; ChildEnv::spawn returns that guard, so callers cannot receive an untracked child or forget to remove its group.
A cancelled session launch emits Exited without a crash.
One session task owns the child, its stdin writer and a process-group guard.
`ByteLines` caps each stdout and stderr line at 4 MiB and survives cancelled reads.
Oversized Claude lines are skipped with a notice, and invalid JSON lines are skipped with diagnostics that reveal lengths rather than raw content.
Valid JSON that cannot decode as an ordinary Claude frame is skipped, logging only its type without frame content.
An undecodable Claude control request with a valid id gets an error control_response and reserves that id for that CLI process's lifetime; an unaddressable request fails the session.
Undecodable Claude control replies, including initialize, fail the session instead of leaving the Ready gate waiting.
Each accepted stdout line is parsed once as JSON, then passes through recording and its provider codec.
Claude additionally decodes subagent frames; Codex deserializes the requests, notifications, items and replies it uses through tolerant wire types.
Unknown Codex methods and item kinds have serde fallback arms, and optional fields have defaults.
The bounded stderr tail is redacted before display and supplies failures when the CLI exits unexpectedly.
Claude Interrupt and Shutdown queue deny replies for every open approval and question with `interrupt: true`; Codex uses the per-kind cancel replies described below.
Decoded::take_outbound places every reply and ordinary control before any interrupt in the same batch, so decoder collection order cannot invert cancellation and interruption.
Both codecs use provider::StopLadder for the active turn's interrupted outcome and both deadlines: group SIGTERM after three seconds, then SIGKILL one second later.
Each codec owns its request cancellation and supplies an Interrupt frame carrying the target and its own protocol line.
SessionControl::Interrupt carries the turn id targeted by a human Stop and a typed StopReason.
Storage failure uses InterruptActive with StopReason::StorageBlocked to stop whichever codec turn is active, independently of actor admission.
Veyra-started turns use their stored turn id; CLI-started Provider turns open under the fresh id reported by ProviderTurnStarted before their first content event.
The actor drops unannounced Provider content instead of inventing an id the codec cannot match.
StopThread and Archive capture the running and pending-admission turn ids before ending their rows.
A targeted Interrupt whose named turn belongs to another thread or is no longer running or pending is a no-op, creating no hold, drain, control or outcome change.
Shutdown and startup recovery use StopReason::AppClosing for their shared turn-end reason.
The codec ignores an interrupt whose target is no longer active, including when a later Provider turn has opened.
Only a matching targeted interrupt cancels that turn's open requests and arms its ladder; active-turn interruptions resolve the target directly from the codec.
The codec returns outbound Prompt, Message, Provider or Interrupt values, with each Interrupt carrying its target turn and supplying its own protocol JSON.
The stdin queue preserves that type, the writer applies its quit commitment check only to Prompt, and it checks an Interrupt's target against the codec's shared active id at each poll until its first byte is written.
An unsent interrupt for an ended target is discarded; its preceding cancel replies remain ordered before any matching interrupt.
Once writing begins, the writer finishes that frame before another frame.
A turn end clears its ladder; an admitted new prompt starts with no active turn or old ladder.
Human interrupts use "Stopped by you"; storage and shutdown carry their own reasons.
A session without a turn never arms that ladder.
SessionControls drains pending controls and closes its receiver on Shutdown or the shared LaunchGate closing state, retaining cancellation through process cleanup.
One drain_and_check_launch gates both initial launch and resume fallback, refusing closing, pending or consumed Shutdown, a closed control channel or a gone event consumer.
Before admitting StartTurn, SessionControls reads the gate's closing state and drains queued controls, so a pending Shutdown or channel closure prevents the prompt without changing cancel-reply or cleanup order.
Shutdown gives queued replies up to one second to drain, then aborts an unfinished writer before group cleanup.
Blocked or failed stdin can prevent delivery; cleanup and leader reaping precede Exited.
Dropping a live group guard immediately kills its owned group and schedules cleanup.
A group still present after the final cleanup deadline produces a bounded warning with its id.
The normal session path discards other cleanup errors.
A supervisor waits for registered cleanup even if the session task panics.
The actor stores pid and group on Ready; Exited ends both running and pending-admission rows before parking.
After StopThread or Archive moves the thread to Stopping, Exited only parks it.

Without an active Veyra interruption or the Claude-only assistant-error latch, externally observed SIGHUP, SIGINT, SIGTERM or conventional signal exit codes produce Interrupted.
Cleanup-generated signals do not change the outcome.
After stdout EOF, the session awaits child.wait alongside stderr completion under the existing 500 ms deadline to observe the natural leader status before signalling cleanup.
A child-wait error is logged and disables that wait arm while stderr retains the same deadline.
It publishes a synthesized turn end before awaiting group cleanup, so post-KILL waits cannot extend the interrupt boundary beyond five seconds.
Exited still follows cleanup, and the actor starts no queued work before Exited.
Other abnormal exits produce Failed with Veyra's bounded, redacted crash text.

`claude::Codec` owns stream-json envelopes, prompt correlation, native block keys, request references, approval and question classification (see Actor decisions) and background-work replacement.
Unknown protocol metadata logs its category without frame content, and unsupported or malformed control requests get an error reply.
Spawns before Ready binds a native id use --session-id with the Veyra UUID; spawns with that bound native id use --resume.
Mode maps to default, acceptEdits or bypassPermissions plus --allow-dangerously-skip-permissions.
An initialize reply can open the Ready gate before the prompt causes system/init to supply native id, version and capabilities.
Prompt UUID echoes and lifecycle acknowledgements separate Veyra prompts from foreign results and Provider-origin work.
The codec retains submitted prompt ids and the command ids of CLI-started Provider turns it has opened for its CLI process lifetime.
Queued or started lifecycle frames for completed commands open no turn; genuinely new command ids still open Provider work.
The seen-command set is discarded with that CLI process's codec.
Uncorrelated CLI work may open a Provider turn after the foreground turn completes.
Assistant error words latch per prompt, and account-related classes park the process.
An active Veyra interrupt overrides that error outcome and suppresses newly arriving diagnostic text, including streamed blocks.
Streamed and final blocks share native keys so final frames do not duplicate delivered text.
The codec supplies a provider-neutral input summary only for the Bash command.
Other tool rows store their input only in arguments, and the transcript formats those arguments generically.
Older stored summaries remain readable.
Each background_tasks_changed replaces the pending set; ambient tasks never keep the thread Working.
Child frames feed the pending root Task or Agent tool's bounded output tail instead of creating root text or turns.
Unknown or finished child parents are ignored, and snapshots retain a running tool's tail.
Expanded tool rows show the live tail while running and prefer the result tail after completion.
Compact, reset, retry, usage-limit and model-fallback frames add notices in Veyra's words.
Only a resume-not-found stderr class starts a fresh native session; other errors retain the native id for Retry.

## JSON-RPC peer and Codex codec

`jsonrpc::Peer<T>` owns the numeric request counter and pending caller tags, with a flag controlling the jsonrpc version field.
Replies return those tags directly.
Replies resolve by id, server requests retain string or numeric ids including zero, and Peer::receive returns replies, requests and notifications to its caller as Frame values.
Peer::eof clears its pending tags; ordinary field destruction releases them when the peer is dropped.
The session independently finalizes active work when stdout closes.
The shared writer in session.rs applies the launch-closing check to prompts and the active-turn check to interrupt frames before their first byte.
Codex uses a reader task over the same cancellation-safe provider::lines::ByteLines reader as Claude.
session.rs reads Codex stdout through jsonrpc::Reader and Claude stdout through ByteLines directly; jsonrpc.rs imports only lines.rs and ProviderError, never the session.
The RPC reader queues four frames, an implementation choice that buffers a short reply burst while applying backpressure.
Four slots hold at most 16 MiB of capped wire bytes or 48 MiB of worst-case lossy UTF-8 text; the reader may additionally retain one decoded frame and its bounded byte buffer.
Reader::next returns text or an error, so an oversize line cannot leak into a caller's impossible branch.
A Codex line over 4 MiB fails the session with the cap in its crash text; invalid UTF-8 decodes lossily and non-JSON lines are skipped.
ACP framing is supported by the peer flag, while no Grok session is implemented.

`codex::Codec` owns its Peer<Call>, app-server wire state, native thread and turn correlation, pending approvals and questions, item output, and protocol retry deadlines.
Conversation distinguishes opening with an optional resume retry from Ready with a native id.
TurnBinding distinguishes Idle, WaitingReply with an owner and optional provisional native id, and Bound with its owner and native id.
Each native turn maps to its owner and one Pending, Started or Ended phase; binding describes admission and correlation, not another native lifecycle phase.
InterruptRequest models an unsent interrupt, its first send, the wait for turn/started and the one retry.
next_deadline combines the shared ladder deadline with a resume retry without claiming every timer is an interrupt.
A session runs one codex app-server process, initializes with Veyra clientInfo without experimental opt-in, then sends initialized and thread/start or thread/resume.
Both thread calls carry cwd, the current model, approvalsReviewer user and the mode's approvalPolicy and sandbox; resume also carries excludeTurns true.
Only the measured no rollout found for thread id text falls back to thread/start and a notice, retaining the same app-server.
An active-writer refusal retries at 250 ms intervals for at most three seconds, then fails with Codex's message and keeps the native id for Retry.
Other resume failures keep that id and fail without starting a replacement conversation.

Every turn/start sends the current model, effort, summary auto, reviewer user, approvalPolicy and sandboxPolicy.
Ask first sends untrusted, read-only and readOnly with networkAccess true; Accept edits sends on-request, workspace-write and workspaceWrite with networkAccess true.
Full access sends never, danger-full-access and dangerFullAccess without a networkAccess field.
Veyra grants Codex no git metadata roots; Codex's sandbox keeps the project's git folders read-only.
Every workspaceWrite policy is exactly `{type: workspaceWrite, networkAccess: true}`, without writableRoots.
A commit under Accept edits docks when Codex asks to run outside its sandbox; Full access commits freely.
A linked worktree's git dir holds commondir, which selects shared config and hooks, and Codex grants whole folders without per-file exclusions.
Granting that directory would let Codex redirect the human's later Git commands, so neither linked nor plain repositories receive an extra grant.
The codec reads thread reply metadata without interpreting the echoed sandbox as a string.
The turn/start reply is Accepted; notifications received before it are buffered and then correlated to the returned native turn id.
A valid own-thread request before that reply attaches to the Veyra turn being started and docks immediately, so a server waiting for consent can progress.
It binds a provisional native turn when one is supplied; an MCP elicitation can omit turnId and still be answered by its request id.
A request for an idle own-thread turn opens Provider work even before turn/started.
Another thread's requests are refused without docking, and a start reply that changes a provisional id fails the session.
The matching turn/completed ends that turn even before turn/started, and a completed native id cannot reopen work.
Another native turn on this conversation can open a separately announced Provider turn; another thread id is filtered before turn routing.
Thread status, token usage, hooks, startup status and unknown notifications do not mark work or add notices.
Own-thread turn completion reconciles its typed item summary by native item key, supplying a final reply absent from individual frames without duplicating already completed or streamed items.
Foreign-thread items and turn summaries are filtered before reconciliation.
Parent collabAgentToolCall rows use a typed action to label wait, spawn, send, resume or stop in Veyra's words.
Typed subAgentActivity started/completed events share one parent tool row keyed by the child native thread id; item completion of a started marker does not complete the subagent.
The row identifies the subagent path in its arguments, ends when a completed marker arrives and creates no Veyra child or Provider turn.
Unknown activity kinds are ignored without reopening completed rows.
Retrying errors add Veyra notice text without displaying raw provider details.
Codex usage-limit frames add a notice only for a known rateLimitReachedType, spendControlReached true, or primary or secondary usedPercent reaching 90.
Null, false and unknown reached kinds do not count as a reached limit.
The 90 percent threshold is Veyra's implementation choice for a near-limit warning; 100 percent or a reached flag is the reached level.
A session reports each level at most once and never reports a lower level after a higher one, even after a usage reset.
Near notices name the highest-usage primary or secondary window, its duration and used percentage when supplied; reached notices name the reported usage, credit or spend limit.
The notice includes the selected window's reset timestamp, or the individual spend limit's timestamp for a spend warning, when supplied, formatted in UTC through the platform module with an explicit Unix-time fallback if conversion fails.

Command and file approvals answer accept or decline; cancellation answers cancel before turn/interrupt.
Command approvals preserve the exact command and the typed cwd consequence.
The project-cwd consequence names the working folder.
An outside cwd and a model-written reason are separate labeled, muted quoted elements rather than strings embedded in Veyra's consequence text. The working-folder quote stays beside the consequence above the scrolling details, with its path in a vertical scroll viewport of at most three lines; the reason scrolls with the command and preview.
ToolCall stores a serde-defaulted veyra_label flag: only Codex's fixed labels are unquoted, while provider-named tools and older stored rows stay quoted.
Both kinds use the bounded display projection; web search and image viewing have Veyra labels.
Consent commands, previews, literal paths and approval tool names show Unicode controls, format and default-ignorable characters as visible ⟨U+XXXX⟩ markers. Commands and Veyra's pretty JSON previews retain newlines; paths and tool names mark them. Multiline command consequences name the line count, including blank lines, above the scrolling details. The shell owns ApprovalScroll, pairing the docked request id with its details and folder scroll handles. A changed request id, including closing the dock, replaces both handles; repainting the same request preserves its offset. An always-visible kit scrollbar exposes actual overflow.
The thread-grant button shows the first line of the marked command, shortened to its available width; execution, native replies and stored grant identities retain the original bytes.
The UI takes ProviderKind for provider-specific behavior; display labels do not select behavior.
Only the exact project cwd may use the existing command grant; every other cwd requires a human decision.
Permissions approvals grant the requested profile for the turn, while Deny and Stop send an empty permissions object.
Questions retain provider question ids and send answers by those ids; cancellation sends an empty answers object.
AgentMessage delivery and question fields are typed separately from server requests.
A valid async questions list becomes ItemBody::AsyncQuestions with the original message text and each title, option list and optional selected index; SQLite's existing item JSON stores that additive shape without a migration.
The transcript renders additional agent message content above the cards with the same item-id markdown cache and shared reply_markdown helper as AssistantText, preserving its full text and paragraphs. Empty messages or messages equal to the trimmed question titles are omitted.
Each card quotes its provider title and option labels and explains that choosing sends a message, first stops active work and continues queued messages.
ChooseAsyncOption is a gpui action, bound to Command-1 through Command-9 and reachable through the debug driver; the engine validates its item, question and option identity and refuses a repeated choice.
One transaction persists the selected index and ordinary User turn.
If work is active, the choice uses the normal targeted Stop cancellation/interrupt path with a QuestionChoice reason, clears any hold and puts the answer ahead of already queued messages while preserving their order.
Turn ordinal updates are applied descending before that insertion to retain SQLite's per-thread uniqueness constraint.
The active session drains before the new prompt is sent; a choice deliberately clears any existing hold, including held queued work, and never requires Continue.
Card eligibility is a projection of item order: any later UserText item in the same thread closes every remaining choice, including after a typed answer.
The engine rejects choices on such cards, and the window disables their controls; no new stored closed flag or migration is needed.
An unnamed Command-1 through Command-9 action additionally requires a card in the thread's latest turn; explicit item actions retain their item/question/option identity.
The question stays in the transcript with Sent text and disabled option controls after its selection.
An MCP elicitation is classified by codex_approval_kind first, then by a message naming serverName and a quoted or unquoted tool with an optional question mark.
It docks with the server and tool, offers no thread grant, and answers only action accept, decline or cancel without persisting provider grant metadata.
Other MCP forms are declined with a notice, and unsupported server requests get -32601.
A command row uses completed aggregatedOutput when non-null, otherwise the bounded outputDelta tail.
A no-active-turn interrupt refusal before turn/started waits for that start and sends one retry within the shared three-second protocol step.
Codex and Claude share process-group cleanup, the stop ladder, shutdown commitment checks and actor recovery paths.
launch chooses its provider setup once, and launch_with shares fixture selection, recorder setup and the ChildEnv spawn boundary.
The run loop chooses one SessionProfile from its codec for stdout reading, malformed-line diagnostics and resume-failure classification.
RootMcp holds either Claude's private Configuration and session credential or Codex's attempt-credential factory.
SessionIo owns the process and recording/credential lifetime, while Process owns the Claude or Codex codec variant.
Each Codex thread/start or thread/resume attempt registers a fresh one-use credential for the existing app-server pid, revoking the previous attempt before sending the next.
Resume refusals revoke immediately, including not-found fallback and every active-writer retry.
McpConfig retains the attempt hashes for inbound, stderr and recording redaction until the root session ends.
Codex's ThreadFrame keeps its credential as McpToken; its provider-owned serialization inserts config immediately before the shared writer writes bytes, and its recording value contains only the token marker.

## MCP relay, authentication and actor tools

relay::run sends VEYRA_MCP_TOKEN from its environment as the first socket line, then platform::relay_stdio polls stdin, the socket and stdout independently with nonblocking descriptors.
Pending writes cannot hide stdin EOF.
Each write direction has an 8 MiB pending-byte cap, an implementation choice that admits a capped MCP frame and a short burst while bounding relay storage to at most 16 MiB; overflow closes the relay with an error.
It reads VEYRA_SOCKET for the connection path and writes no token-bearing file.
When stdin ends, the relay shuts down both socket directions and returns even when a socket write or stdout write is blocked.
This also ends a Codex relay in its separate process group when the app-server closes or dies.
A live native-binary codex-cli 0.162.1 run with a stand-in stdio server measured the direct-child, own-group shape and exit on stdin closure.
A live Codex 0.162.1 run at daf400c has since reached the production Veyra relay in Ask first and Full access, with accepted connections and list_providers results.
Its measured relay parent was the registered app-server pid, and each relay had its own group.
Authentication still requires that direct-parent shape; an intermediate process is refused.
The relay starts no window, takes no instance lock and emits no protocol diagnostics to stdout.
McpToken contains 32 platform-random bytes encoded for the environment, and Debug prints McpToken(***).
Server::for_spawn registers a pending credential before its spawn or existing-pid registration callback, releases the registry lock before configuration and the launch gate, then publishes the actual CLI pid through a watch channel.
Authentication waits up to three seconds for that registration and checks the still-current credential under its lock.
Revocation can remove a pending credential while launch runs; no credential lock is held across the launch gate.
Only the hash remains in the credential registry, and a credential permits one connection.
The plaintext token remains readable in the relay environment for the session, so the executable, direct-parent and single-use checks all remain necessary.
A later spawn or Codex thread-open attempt for the same thread revokes its earlier credential; a revoked open connection closes on its next input line.
Claude Session Drop removes its credential by hash and Codex McpConfig Drop revokes its thread and epoch, and Actor::exited also revokes the thread and epoch before committing exit.
Server::bind requires a private 0700 data folder and uses platform::bind_async_socket to unlink a stale veyra.sock and bind it at 0600.
The server gets the peer pid with LOCAL_PEERPID, its direct parent with proc_pidinfo and its executable path with proc_pidpath.
Authentication compares the peer executable with the supplied relay command and its direct parent with the registered CLI pid before consuming the token hash.
Engine::launch supplies the prepared ChildEnv relay_command, or std::env::current_exe by default, to Server::bind before starting its file-backed actor.
relay_command is a trusted host input, never loaded from environment variables or provider frames; the public prepared-environment seam lets application integration tests use Cargo's exact production binary.
In-memory test engines have no data folder and do not bind a socket.
Tests supply the built Veyra executable and reach it from the fake CLI's direct-child relay.
Accepted and refused connections log the thread id when the token identifies one, and tools/call logs the thread and supported tool name without arguments.
Unknown tool names log as unknown.
Socket lines reuse the provider's bounded 4 MiB ByteLines reader.
The authentication line has a three-second bound, Veyra's implementation choice to prevent an idle unauthenticated client from holding the connection open indefinitely.
Pending launch registration also waits three seconds, Veyra's implementation choice to allow launch publication while bounding a stuck spawn; timeout and closed registration log the known thread and refusal reason.
The server serves initialize, ping, tools/list and tools/call over newline-delimited JSON-RPC.
After authentication, a reader task queues at most four frames and separately reports EOF.
Four slots are an implementation choice for a short MCP request burst, corresponding to at most 16 MiB of capped wire bytes or 48 MiB of worst-case lossy UTF-8 text, plus one reader frame and its bounded byte buffer.
Queue overflow closes the connection and revokes its credential rather than waiting for queue capacity.
EOF and closure also cancel a blocked socket reply write.
EOF revokes its credential and closes the connection even while a tool reply is waiting on the actor, and dropping the connection aborts its reader.
Every unsupported request, including Claude's server/discover, receives -32601 immediately, and notifications receive no reply.
A live Claude 2.1.296 run measured the relay starting directly, supporting the executable and parent checks.
That run observed system/init list veyra as connected with source dynamic after the first user message.
The same run observed initialize instructions from the start and tool descriptions deferred until ToolSearch loaded their schemas.
A CLI version that starts the relay through an intermediate process fails the direct-parent check; refusals are logged and the server stays unavailable.
Instructions retain the complete first-call rules byte for byte.
The start_thread description carries the full family rules once; each other tool keeps its own base description and one short pointer to that carrier.
The unchanged instructions retain the same base descriptions and family rules: ownership, depth one, four open children, sixteen total family threads, the permission ceiling, prompt and report caps, idempotent request ids and report wakes after the current turn ends without polling.
A live Codex 0.162.1 run did not expose server instructions to the model, so descriptions supply those rules before its first call.
Configuration uses platform::private_dir and platform::private_file to write the sole token-bearing file, 0600 DATA/run/THREAD/mcp.json under a 0700 folder.
Its mcpServers.veyra entry names the relay command, mcp argument and env bindings VEYRA_SOCKET and VEYRA_MCP_TOKEN.
Its system_init handler removes the file when veyra is listed with any status, logs a non-connected report with its thread id, and Drop removes it if the session ends first.
An unlink failure in system_init fails the root session rather than retaining a reported-connected config.
provider::session::SessionIo owns an optional Claude Configuration and a token hash throughout the root session.
launch gives --mcp-config PATH only to roots, including resumes and fresh replacement sessions, without adding the token to the root CLI environment.
Child sessions receive neither the config argument nor a relay credential.
Codex roots carry the relay command, arguments and credential environment in dotted mcp_servers.veyra.* keys of thread/start.config or thread/resume.config.
They create no token-bearing config file and add no token to the app-server environment or argv.
A queued tool request carries the authenticated thread, epoch, actual CLI pid and hash; the actor checks the current structured session and live credential before any decision.
The CLI actor refuses a tool request without its originating session; only scripted unit fixtures use that seam.
The token never enters argv, logs, evidence, fixture text or crash text.

Actor::handle_inner sends every tool decision through tools::decide, the existing ChangeSet commit and the existing effects path.
ProviderKind::is_built owns provider availability for the spawner, tool catalog, start validation and start_thread schema.
list_providers reads the shared readiness catalog and offers built adapters with Ready and a resolved path, their models, the caller's mode ceiling and remaining fan-out.
start_thread offers ready Codex as well as Claude, and refuses Grok with the unsupported-version rule and a list_providers call.
Probes populate that catalog and Engine::child_env shares it with the actor.
start_thread creates a child lineage, first task turn and UUID-v5 task keyed by caller and start_thread:client_request_id.
get_task reads only the caller's task and reports the child's Status::words with its settled summary when present.
message_thread creates a task on an owned child and reports started or queued with the derived wait reason.
stop_thread uses the existing child stop and settlement helpers with Stopped by its parent and leaves the parent unheld.
Identity is supplied by the authenticated connection, never by a self argument.
Ownership and a running-turn gate apply inside the actor before any tool decision commits.
A child may not start a thread, and the store's lineage constraints independently refuse grandchildren.
The derived permission order enforces child mode <= parent mode, with the parent's mode as default.
The fan-out cap counts distinct children with open tasks, with a limit of four on starts and follow-ups.
A deduplicated retry or a message to a child already occupying a slot needs no extra slot.
The family cap counts all rows sharing the root, including archived rows, with a limit of sixteen.
A typed Tool enum owns the names, schemas and descriptions; shared constants provide the checks and their stated limits, including prompt and report caps.
A retry compares the stored first user item and child provider, and message_thread also compares the requested child id before returning a stored task.
Reusing a key with different text, provider or target is refused, while the two tool names create separate key namespaces.
Prompts and follow-ups are capped at 100,000 Unicode characters.
Refusals name the rule, applicable count and limit, and a next tool with the identifiers or argument shape needed to recover.
start_thread stores two child-guidance sentences before the parent's prompt in the child's first task item, so the transcript, database and provider receive the same text.
Retries compare the parent's original prompt after removing that exact stored guidance prefix; later task and user items stay literal.
The existing settlement path waits for no running or queued child turn and no background work, or handles Stop and recovered-child Continue.
It coalesces reports into a queued wake and records Pending delivery in the same transaction.
After completing buffered text, the actor writes each newly settled task's summary and optional source item id from the same latest assistant item in that settlement transaction.
TaskState::Settled owns the source link beside summary and delivery, and tasks.source_item_id stores it; no assistant item means no source.
A settlement while a wake runs creates a successor wake.
Wake text frames each child's final assistant message as a report and caps its summary at 4,000 characters.
For a task that did not complete, the wake adds an Outcome line using TurnOutcome::words shared with the window, capped at 200 Unicode characters including its prefix and a trailing ellipsis when cut; the window's outcome words are unchanged.
Veyra chooses that outcome limit to keep provider-supplied failure text from dominating or repeating the carried report.
For every task outcome, the wake says when the child left no report text; completed reports with text keep their existing content.
The ordinary start rule waits for the parent to be idle and unheld, so a wake never interrupts or steers.
The first Accepted for a turn changes task delivery to Delivered and logs its thread, turn and acceptance time; the actor ignores later Accepted events using a runtime turn-id set, whose entry is removed when the ended turn is installed.
Any wake ending before Accepted moves pending reports into a new queued wake; for an unarchived root delivery is at least once until acceptance, can repeat, and is final once Delivered.
Parent Stop preserves a held wake, and a user stop or archive of one child holds the parent as ChildReport without replacing an earlier hold.
A kept child settles into its archived root's queued wake, which never starts: the archive's Stopped hold, or an earlier preserved hold, refuses it first; WaitReason::Archived still refuses it if that hold clears, and the window offers it no Continue.

## Platform and keep-awake

The platform module owns macOS file modes, locks, sockets, login-shell PATH, executable checks, group setup/signals, process inspection, idle-sleep assertions and Terminal opening.
Each agent and probe spawn attempts to set group nice 10 and logs failure while continuing.
The actor attempts to hold one caffeinate guard while any thread is Starting, has a busy turn or has non-ambient background work.
An assertion-helper spawn failure is logged without stopping work.
The pure needs_awake predicate controls that guard, whose drop stops its owned caffeinate child.
Platform module selection rejects unsupported targets with compile_error.
The textual platform_boundary test is a guard, not proof against every indirect OS call; review still checks indirect commands and strings.

## Readiness, fixtures and recording

Startup, Check again and Retry probe each provider independently on the runtime while the engine remains open.
RecheckProviders and Retry refuse rechecks once Engine::is_closing reports closing, and stale probe replies cannot retry a turn during Quit.
`Probes` owns each provider's cancellable task, refuses scheduling while closing and checks cancellation after awaiting a predecessor.
The Claude probe uses one check_cancelled helper before both version and initialize spawns, including after version cleanup; ChildEnv::spawn owns the closing check for each launch.
A replacement cancels and waits for the earlier task, including its process-group ladder, before starting.
Shell generation numbers reject stale probe replies.
Claude has no probe timeout; a hung Claude stays Checking while the other providers become Ready.
Claude resolves its path, reads its version, launches stream-json in the private data folder with no session flag, sends initialize and matches the request_id without a turn.
It reads models and optional account, then ends and waits for the probe process through the same group ladder.
A missing binary is Not installed.
Except for a missing binary or the captured signed-out signal, every probe failure or inconclusive reply reads Ready.
An account with apiProvider and tokenSource but no signed-in identity fields reads Signed out, following Veyra's recorded Claude signal.
Missing or empty account remains unknown and Ready.
Both version and initialize reads reuse capped byte lines, and stderr is drained so a chatty probe cannot block its stdout reply.
Codex resolves its binary, initializes one app-server, sends initialized, account/read and model/list, and reads every advertised model page without starting a thread or turn.
One five-second timeout covers the whole Codex probe exchange, including every model page; cancellation and completion clean up through the shared process-group guard.
Its version comes from initialize.userAgent, model values from model/list, and effort levels from each model's supportedReasoningEfforts.
An explicit null account with requiresOpenaiAuth true reads Signed out; otherwise an absent account remains unknown and Ready.
A Codex probe failure other than NotInstalled follows the same unknown-and-Ready fallback as Claude.
Grok stays a Ready placeholder whose spawn returns NotBuilt, fails the oldest queued turn and holds the Starting thread as Stopped.
The Shell holds raw probe account metadata in memory.
Dump exposes only account presence as known or unknown, never its value.

`fixture.rs` defines Veyra's transcript_start, expect_outbound, emit_inbound, emit_raw, touch, spin, exit, mcp_start, mcp_stop, mcp_send, mcp_expect, record_relay_pid and record_argv records.
Touch creates a file under the fake CLI's cwd and rejects absolute paths and parent components.
Spin burns CPU for its supplied milliseconds.
Headers name provider, protocol, CLI version, scenario and source.
The matcher compares JSON exactly except <any>, <cwd> and first-use <id:name> bindings.
emit_inbound supports after_ms; replies expand the same bindings.
emit_raw decodes base64 bytes and writes them repeatedly for invalid-byte and oversized-line cases.
The fake CLI ignores SIGTERM when its header sets ignore_sigterm.
`veyra-fake-cli` consumes one fixture and exits 3 with a content-free mismatch diagnostic.
With --mcp-config, the fake CLI binds the config command, arguments and environment in memory, refuses a root environment carrying VEYRA_MCP_TOKEN, and mcp_start launches the relay as its direct child.
record_argv stores only the fake CLI argv under the same path restrictions as touch.
A Codex thread/start or thread/resume binds the dotted config keys for the same MCP records, and its fake relay has a separate process group.
The mcp_stop fixture record closes and reaps that owned relay before a later attempt starts another.
record_relay_pid stores only that owned relay pid under the same path restrictions, allowing process-exit assertions.
Capture-derived fixtures come from Veyra's own CLI recordings; other cases are Veyra's scripted wires.

The debug-only branches of `ChildEnv::load` are the only application read site for VEYRA_CLAUDE_BIN, VEYRA_CODEX_BIN, VEYRA_FAKE_FIXTURES, VEYRA_RECORD and VEYRA_REAP_PIDS.
The provider overrides replace their matching Claude or Codex binary.
Each Engine owns its lane registry through its shared ChildEnv.
New threads register in creation order; each thread's spawn E uses tN-sE.jsonl, with probe.jsonl for Claude probes and probe-codex.jsonl for Codex probes.
Stored threads register when first spawned in that run.
Missing replay files fail spawn with their path; the child gets the absolute singular VEYRA_FAKE_FIXTURE only in debug builds.
The standalone fake executable reads that singular variable.

The debug Recorder creates private 0600 captures through platform::private_file.
It binds the supplied cwd, optional home folder and known ids.
SessionIo sanitizes inbound and outbound recording values with its token hashes before calling Recorder::line, replacing any exact token with <id:token>.
The same hashes sanitize provider inbound values and stderr before UI, storage or crash text receives them.
The codecs hand the recorder typed native-id and version metadata decoded from their thread reply or system/init; recording protocol labels come from ProviderKind.
ChildEnv reads HOME once and supplies it to the recorder; `<home>` is a capture placeholder, not a matcher binding.
It replaces credential fields, redacts secret-shaped text, replaces hook output with a placeholder and keeps account keys with masked values.
Claude probes are recorded as probe.jsonl; a later probe replaces that capture only after the earlier probe has ended.
Codex probes are not recorded; probe-codex.jsonl is a replay fixture only.
It records inbound/outbound wires and exit, updating Claude's header version from system/init.
Codex captures use the json-rpc protocol label, bind returned native thread ids distinctly and update their version header from thread reply metadata.
Hash-redacted <id:token> bindings survive credential-field masking without retaining the credential.
The shared crash redactor applies exact token, Bearer word, credential-name value, long sk-/xai- word and URL password matches in that order.
The fixture privacy sweep also rejects concrete home paths, email and account-id shapes, and a neutral planted needle in raw or assembled text.
Recorder and override read sites are compile-time debug-only; release string checks prove their exclusion from the application binary.

## Window projection, transcript and Settings

`Shell` owns the engine handle and read-only projection, window navigation and input, readiness tasks and transient presentation state.
`MainView::Chat` carries its thread id; Settings keeps an optional return thread.
It subscribes before requesting snapshots.
The subscription starts with SettingsChanged, projects, threads, turn rows, task state and delivery rows, and one status per thread.
It also sends StorageBlocked if storage is blocked and RecoveryNotice when startup could not read its storage receipt.
Every thread-scoped initial row advances that thread's counter.
The gpui pump drains every queued event into one entity update, applies it and notifies once.
AppState counts applied batches and the largest measured gap between text-bearing batches.
Debug-only VisiblePerf counts applied TextAppended batches for the selected Chat thread, ignoring other threads' text; batches buffered behind a loading snapshot still count.
VisiblePerf and AppState each keep a BatchGap value for their largest gap and previous timestamp, sharing its type while retaining their own counts and resets.
VisiblePerf resets its count, gap and previous-batch timestamp on selection changes and on that thread's TurnStarted.
AppState clears only its previous-batch timestamp on any TurnStarted, preserving its lifetime batch count and largest gap.
Debug-only TranscriptOpen records the Snapshot request identity and time, marks its installation, and ui::transcript::render stamps the first render of that installed snapshot.
The dump exposes perf.visible and perf.transcript_open_ms, which is null until an open and replaced on every open.
A theme-cadence task refreshes Working time once a second while any thread is Working, including an unselected row.
AppState keeps a per-thread activity clock from TextAppended, ItemAdded, ItemCompleted and RequestOpened; TurnStarted resets it to that turn's start.
quiet_for derives Quiet for N min after ten minutes without activity while Working, with no unfinished tool row and no pending dock request.
The header and sidebar retain the title beside that hint and reserve its full text width, truncating the title when space is narrow; the engine status remains Working.

Opening a transcript requests an actor snapshot, never SQLite from the UI.
Every snapshot request has an identity.
While it is pending, AppState fences that thread's item events, retaining them and updating global thread metadata.
Reselecting preserves buffered events; only the current request may install the items and fence.
An enqueue failure releases only that request's fence, replays buffered events and shows a load notice.
It installs stored items plus buffered text and replays only item events above the snapshot counter.
Snapshot installation and fence release do not await the header's Git branch lookup.
Later stale transcript events are ignored, but metadata stays on the ordered event stream: an items-only snapshot does not carry native-id, turn or live-state rows and cannot fence those updates.
This joins the item snapshot and event stream without duplicated or missing text.
When the selected Chat thread becomes archived, both the event pump and acknowledgement path call leave_archived_thread to open New thread with the stored default mode and reset its draft, picker, notice and branch.

The sidebar reads projects in creation order, then each root and its children on a one-pixel guide, with the root glyph derived from family_status.
An unarchived child whose root is archived becomes a top-level row and keeps its own status.
OpenThreadMenu exposes Stop thread, Archive and, for a root with unarchived children, Archive, keep child threads.
All resizing paths dispatch SetSidebarWidth.
The header reads title, status/elapsed time, project and a separate Git branch task on the tokio runtime.
A child shows the root title, a slash and its title; SelectThread links back while that root remains unarchived.
An archived root's breadcrumb is plain text, and a parent in NeedsYou adds its provider's needs-you hint.
Branch lookup starts once the selected Chat thread's cwd is known, either at selection or after an event batch supplies its row.
The retained task prevents another lookup until Chat is left or a thread is reselected.
That child inherits the app environment, has kill-on-drop and no timeout.
Entering Settings clears the displayed branch and lets an in-flight lookup finish; its selection and view checks discard that late reply.
Other view changes and Quit call `forget_branch` to cancel the task and clear the displayed branch.
Dropping the Shell also cancels the task, and its reply checks the selection identity.
The centred transcript uses a window-owned MessageScrollerState over gpui ListState and selectable Markdown TextView.
The transcript and its expansion behavior live in ui/transcript.rs.
The Started N agents card, singular Started 1 agent, is the first derived list row and lists every child, including archived children whose Open is disabled.
Each row uses its child's own status glyph and words, SelectThread as Open, and StopThread as Stop only while the shared stop_target rule names a turn and the child is unarchived.
TaskCard rows show Task from the parent provider; Task and Wake turns label the parent title and distinct reporting children respectively.
A Delivered task labels only its named source assistant row Report delivered, including when queued tasks settle with a later task's reply; later identical user text cannot move the badge.
A task without a recorded source shows no delivery badge, including older stored tasks and a child stopped before producing an assistant item.
The row renderer and dump use the same report_delivered predicate over the task's stored link.
Turn::recency orders the transcript, ThreadView.last, failure cause and Retry by actual start, or end time for a turn that ended without starting, with ordinal for ties and queued turns after started ones; stored queue ordinals retain their admission role.
The actor attaches a session notice to a running turn when one exists, otherwise the latest started turn; queued and never-started turns cannot take a notice.
Startup storage-loss notices use the same rule on the reconciled rows, so an attempted-start turn recovered as Interrupted can take its notice because it keeps its attempted start time.
When no eligible turn exists, the shared notice selector logs the thread id and notice kind without the notice text.
ReplyLineBreaks applies to assistant reply rows and uses the kit's Markdown AST hook to turn prose soft breaks into rendered line breaks, preserving lists, emphasis, tables and code, with the original Markdown retained for source copying.
AppState caches each thread's ordered item ids and row positions, rebuilding them on item insertion, turn metadata and snapshots.
It fills shared markdown source strings for assistant and reasoning rows before they enter the cached order, refreshing only the changed item and removing archived rows.
The row renderer receives that row's cached markdown source without rebuilding it.
The list reads items and turns directly from the projection instead of cloning the transcript on every render.
The first family card inserts before existing list rows; text remeasurement and expansion translate cached item positions past that row.
AppState::text_rows selects only a text-only pump batch's changed rows in the open thread for remeasurement.
Other batches still remeasure the list for structural, outcome and queue-label changes.
The inline streaming caret remains part of the markdown source, so a growing assistant row still takes the kit's full-replacement parse path.
Failed tools show their last nonempty output line, truncated to one row at its width.
The window and dump share row_outcome, which annotates only each ended turn's final item.
A user scroll detaches following; new text, requests and tool rows preserve that state.
Scrolling within text.read.line_height of the bottom or choosing Latest reattaches.
Latest appears only while following is detached and content remains below.
Expanding a row remeasures it without moving its top anchor.
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
Codex uses the same picker and composer, with model labels and effort choices read from its probe.
Its commands, file changes, permissions, MCP approvals, questions and retry/limit notices use the existing dock, question card and transcript rows.
A Claude root's Codex child uses the same task card, breadcrumb and family projection.
CreateThread sends the draft configuration; subsequent Send targets the selected thread.
On an open thread, model, effort and permission chips send Configure, which is refused while work runs and parks an idle session.
Enter submits, Shift+Enter inserts a newline, and the textarea propagates Command+Enter to the bound Send action without a second subscription send.
New thread resets text and reads the current stored default.
Settings occupies the main column, opens through OpenSettings/Menu/Command-comma and exposes one three-mode row.
SetDefaultMode publishes SettingsChanged; Escape or thread/project selection leaves Settings.

`ui/actions.rs` owns actions, the native menu and one bindings table used by both key installation and shortcut_chip.
Ctrl+C is literal control in the composer and preserves its draft while interrupting the turn.
The approval dock (`ui/composer.rs`) renders only the codec's typed ProposedAction and does no disk work to render it.
The chat column bounds the composer by its available height; the transcript gives up space before the dock.
Flex layout measures the hold row, request count, consequence, wrapped decision buttons, focus hint, input and toolbar before shrinking the details to the remaining height, capped at composer_max_height (240px).
The command or file path, model-written reason and bounded argument preview share that details scroll area, which keeps at least one code-text line.
The consequence and any outside working folder sit above it, and Allow and Deny stay below it.
The folder's viewport keeps at most three label-text lines; a tall message draft can shrink to the composer's minimum height.
At both theme window sizes, the approval matrix retains the long folder's three lines and at least one code line even with all fixed controls present, so no smaller-folder fallback is needed.
Folder and details use one bounded-scroll helper with independent kit scrollbars, visible only for actual overflow.
Shell owns both handles and replaces them when the docked request id changes, so each new request opens at the top with its full marked path available by scrolling and no elision.
The stable folder element id does not own that reset.
The dock shows 1 of N for multiple pending requests.
Approval chords and number keys answer only while the dock has focus; the composer keeps Command+Return as Send.
Consent buttons carry their displayed request id, and question choices also carry the displayed question index.
Shell::dock_target selects the request and question index for dock rendering, consent buttons, the debug dump and keyboard handlers.
Button ids contain the request and question identity, so a repaint to a different target cannot complete an earlier mouse press.
Keyboard handlers retain the focused dock's rendered target.
Respond rejects an expired or missing request, and a question choice rejects an index that has already advanced, without selecting another request.
Because a docked request implies a running turn, composer Command+Return with an empty draft dispatches Stop, which interrupts the turn and expires the request.
Approval buttons show their answer chords only with dock focus; otherwise the dock offers Command+Shift+Y to focus it.
Requests never take focus automatically, and answering an approval or question keeps dock focus while another request remains.
After the last response, focus returns to the composer once no other pending request remains.
Approvals and questions share the same unfocused dock prompt and FocusDock chord.
The focused dock has an accent border.
The focused approval chords are Command+Return, Command+Option+Return and Command+period.
Grant button captions keep whitespace in their visible first line, add an ellipsis at a newline, and truncate to their width.
Their accessibility labels retain the whole original caption.
NextNeedsYou scans sidebar order for NeedsYou before Failed, wrapping once and including child rows.
The pure stop_target function selects a Running row for the selected thread, or its published WindowThread pending_turn when no Running row exists.
The composer shows Stop only when that function names a turn and its draft is empty.
Shell::shown_stop returns that rendered target while the trimmed draft is empty; the composer and dump read it, and Send dispatches Stop for it before checking provider readiness.
A Refused admission with no Running row has no Stop target; held reasons and Continue remain separate controls.
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
`scripts/bundle-app.sh` builds a macOS app bundle with dev.veyra.app identity and signs it ad hoc, because macOS refuses notification authorization to an unsigned bundle.
System permission and native banner clicks require operator verification.
The debug-only DebugNotificationResponse action passes a banner tag to the same notification_response handler as the native callback, allowing stale-tag routing checks without OS delivery.

Provider display text uses the engine's pure text::display_line projection, with its own 400-character input cap, a trailing ellipsis when cut, collapsed whitespace and removed Unicode format and default-ignorable characters.
Dock reasons, transcript provider tool names, async card titles and options, and family/plan labels use that projection with quotes and the existing truncation mark; reasons have their own labeled, muted element.
Literal consent paths and approval tool names instead retain their full marked consent projection in both action text and labeled quotes; commands retain their marked text with heredoc newlines.
The debug dump exposes provider_text entries in approval docks and display_questions plus choices_active on async items beside the unchanged raw body.
Those fields use the same display projection as rendering, while raw options remain available for exact protocol assertions.
The debug tool_label and dock grant_caption fields share the row and grant-button rendering helpers.
The debug-only DebugScrollApproval action moves the actual details handle used by wheel scrolling; details_scroll exposes its offset and maximum only when the scroll state belongs to the currently docked request, without changing consent state.
Raw option strings remain in the stored card and are selected by validated indices for UserText, so display cleanup cannot change the sent message.
Commands and argument previews retain their code styling with visible Unicode markers; their underlying consent identities keep the exact bytes.

## Debug protocol and verification trust

The control module, ctl argument, driver socket references and UI dump/input methods exist only under cfg(debug_assertions).
The driver uses DATA/control.sock at mode 0600 inside the private directory.
It trusts same-user clients and has no additional authentication.
One socket thread serves tagged JSON-line commands serially; a gpui task dispatches through the window.
Named actions dispatch through the window on the Shell's stable root focus; keys use the focused path.
Quit uses gpui's deferred Window::dispatch_action.
Replies acknowledge dispatch rather than completed engine work, so verification polls the resulting window state.
Command lines are limited to 65536 bytes, replies to 4 MiB, socket reads/writes and UI-reply waits to two seconds, and the request queue to 16.
The reply limit is Veyra's implementation choice to carry the 5,000-item snapshot dump while retaining a bounded client allocation; the smaller command limit remains unchanged.
An incremental same-user sender can keep a line read occupied inside the per-read timeout.
A full queue rejects before enqueueing; a reply timeout does not cancel an action already queued.

Action accepts optional JSON data, keys parses before dispatch, text uses the focused textarea's public input handler, and quit dispatches ConfirmQuit.
Dump reads identity, theme, view, engine status rows, selected native id/configuration/failure and rendered header status and branch, items, composer, readiness, settings and pump counters.
Each sidebar thread and selected thread includes lineage, and tasks includes each received task row with its state, delivery and optional report source.
Debug-only item delta_count counts actual text-stream deltas in the current process and survives transcript snapshots; it is not persisted across app restarts.
Debug counts are removed when their items leave the projection or the thread is archived.
The driver adds the engine's schema_version without opening the database.
Wait is client-only and polls dump every 50 ms, for 30 seconds by default.
It exits 0 for status, 1 for timeout or a driver/connection error, 2 for bad usage and 3 for a missing or archived thread.
A timeout writes `veyra ctl wait timed out:` to stderr; a driver or connection failure writes `veyra ctl driver error:` with its cause.
No wait server command exists.

[verify-veyra](.agents/skills/verify-veyra/SKILL.md) owns the fresh-shell launch, driver, capture, evidence and cleanup procedure.
Its fake lane isolates data and uses only the fake CLI; an explicitly authorized live lane records actual provider wires.
Proof stays outside disposable scratch, and driver and cleanup actions target only processes this run started.
Captures require the owned window to remain on-screen and frontmost; the guard prevents covered or stale frames from counting as proof.
Release ignores the debug reap filter and can signal any validated orphan, so the skill keeps release checks away from another run's deliberate orphan.
Dump's awake reports whether the actor retains the guard, without checking the child or OS assertion after spawn.
Dump also reports each known live CLI pid with resident_bytes.
Debug perf reports commits, actor_max_ms, actor_busy_ms, mailbox_free, repaints and max_batch_events.
mailbox_free is the command sender's unreserved channel capacity; slots assigned to waiting sends count as used, so it reads 0 while any send waits.
actor_busy_ms is the elapsed time of the message currently being handled, or zero when idle.
Dump includes every request row the window has received, including its state, plus quit_sheet, window_active, notifications.posted and transcript.items with transcript.follow.
The composer dock adds action_text, consequence and preview from the same formatter the window renders.
DebugStallUi blocks the window thread for its supplied milliseconds so the pump can be measured under a controlled stall.

## Source map

- [Entry and runtime](crates/veyra/src/main.rs), [engine handle](crates/engine/src/lib.rs), [data folder](crates/engine/src/data_dir.rs).
- [Actor](crates/engine/src/actor.rs), [decisions](crates/engine/src/commands.rs), [domain/indexes](crates/engine/src/domain.rs), [live variants](crates/engine/src/live.rs).
- [Relay](crates/veyra/src/relay.rs), [MCP socket and config](crates/engine/src/mcp.rs), [tool decisions and guidance](crates/engine/src/tools.rs).
- [Storage/schema](crates/engine/src/store.rs), [recovery](crates/engine/src/reconcile.rs), [events/snapshots](crates/engine/src/events.rs).
- [Provider boundary](crates/engine/src/provider/mod.rs), [environment](crates/engine/src/provider/env.rs), [sessions/probes](crates/engine/src/provider/session.rs), [Claude codec](crates/engine/src/provider/claude.rs), [JSON-RPC peer](crates/engine/src/provider/jsonrpc.rs), [bounded lines](crates/engine/src/provider/lines.rs), [Codex wire types](crates/engine/src/provider/codex/wire.rs), [Codex codec](crates/engine/src/provider/codex.rs).
- [Fixture format](crates/engine/src/fixture.rs), [fake CLI](crates/engine/src/bin/veyra-fake-cli.rs), [recording/redaction](crates/engine/src/record.rs).
- [Window/pump/fence/branch](crates/veyra/src/ui/mod.rs), [transcript](crates/veyra/src/ui/transcript.rs), [composer](crates/veyra/src/ui/composer.rs), [Settings](crates/veyra/src/ui/settings.rs), [actions](crates/veyra/src/ui/actions.rs), [theme](crates/veyra/src/ui/theme.rs).
- [Banner decisions](crates/veyra/src/ui/notify.rs), [macOS app bundle](scripts/bundle-app.sh).
- [Process supervision](crates/engine/src/process.rs), [Platform mechanisms](crates/engine/src/platform/macos.rs), [selection](crates/engine/src/platform/mod.rs), [boundary guard](crates/engine/src/platform/boundary.rs).
- [Driver](crates/veyra/src/control.rs), [name coverage](crates/veyra/src/verify_map.rs), [verification skill](.agents/skills/verify-veyra/SKILL.md).

## Verification

multi_agent tests exercise actor rules, idempotent tasks, guidance, shared status words, settlement, held and successor wakes, grants, uncertain-admission recovery and retained writes and recovery notices.
mcp_socket uses the built relay and a fake CLI that records only its own argv, then checks foreign-peer refusal, the production relay with a wrong direct parent, a second relay from the same CLI reusing its token, private files and socket modes, RPC method fallback, instruction coverage and config cleanup.
Removing either the parent or one-use check makes its isolated relay regression fail.
mcp.rs launch_lock_tests blocks a launch closure and exercises revocation concurrently, with a bounded wait proving revocation does not wait for that closure.
mcp_socket also exercises production root session config lifetime, non-connected init cleanup, pre-init exit cleanup, child argv isolation and token redaction.

Real-relay socket and Codex family tests live in the application package, which receives CARGO_BIN_EXE_veyra and a fixture binary target from Cargo.
Both fixture binary entrypoints call the same engine fixture::cli implementation; engine-only fake-session tests resolve their helper beside the current test profile instead of a fixed target/debug path.
No stale-binary timestamp guard or preparatory workspace build is required; the empty-target workspace test covers that ownership boundary.

Workspace tests cover private paths and modes, launch locking, platform_boundary, command-table behavior, recovery, storage, sessions and theme contrast.
replay_claude uses the real session task with the fake CLI for text and tools, prompt correlation, provider-turn completion, resume, crash and error outcomes, background replacement and readiness.
It covers approvals, questions, unknown requests, Stop with a pending approval and late interrupts.
It also checks assistant-error parking, interrupt outcomes and ladder cancellation, subagent isolation against the live Agent recording, notices and malformed-byte recovery.
The late-Stop actor/codec and real-session tests complete a user turn, open a later Provider turn, then check that the old Stop leaves the later turn Completed, alongside a control without Stop.
provider_interrupt_targets_provider_turn_and_preserves_cancel_order checks a stale Provider turn id and cancel replies before a matching interrupt.
The queued-prompt actor/codec and real-session tests preserve A Completed, Provider B Completed, then C Completed with C's own reply.
A second actor/codec ordering ends B before the pending admission and checks that C's prompt is not dispatched twice.
A Stop before refusal preserves the stopped prompt's end and allows the later Provider turn and queue to settle.
stop_targets_reported_provider_turn stops B with the real session's ProviderTurnStarted id and requires Interrupted.
Storage-failure tests require an active-turn control before the Provider row is registered and prove that an unregistered CLI turn ends with the storage-blocked reason.
Actor tests cover retirement before queued work, the Stop drain, Provider-tail rejection and skipped text-only transactions.
Mailbox tests require a full-channel send to wait and two waiting commands to reach the actor in order, even when the second future is polled first after capacity opens.
Tests in tests/session_events.rs require duplicate Ready and admission-only changes to retain their runtime and metadata effects without another commit.
A projection test checks text-only row selection for the open thread, another thread, mixed batches and items absent from the order.
A projection test checks cached row order, streamed text, completion and archive cleanup.
Window regressions check visible text counting and both resets, stalled-visible gaps despite other-thread text, first-render open timing and replacement, left-aligned titles and one-line quiet rows, conditional family Stop, recency, archive eligibility and reset, and report-source anchoring.
The multi-agent recipe's child-task replay checks that the provider receives the stored guidance.
perf_snapshot_large is ignored by ordinary workspace tests, seeds one file-backed thread with 5,000 items across fifty complete tool-heavy turns, times one actor Snapshot and prints the database path it retains for a window drive.
Its normal seed regression checks the stored shape, and the stream-pause fixture regression checks one 300 ms delay after 1,000 of the 2,000 deltas.
Multi-agent's Probe smoke measurements recipe records the pause's visible gap and the seeded transcript's first installed render, copying the retained database into the stopped app's short disposable data folder before reopening it.
Shutdown tests check that Quit during a drain, Stopping or missing-session recovery starts no replacement session and keeps queued work held after reload.
An actor decision test checks that shutdown exits propose no restart or queued-turn start, preserving epochs and earlier holds.
A storage test checks that a Write tool row stores its content once.
Platform tests check the two-second login-shell bound, inherited-pipe deadline and group nice value.
Session tests check process exit classification and cleanup ordering.
shutdown_during_missing_resume_never_launches_replacement and shutdown_before_session_launch_starts_no_provider check launch refusal, each with an uncancelled stand-in positive control.
pending_shutdown_preempts_queued_turn_on_ready_session checks queued Shutdown, control closure and shared closing before prompt admission, with an uncancelled prompt control.
shutdown_request_* tests check backlogged commands, Ready and every retiring-session restart before the mailbox Shutdown.
cancelling_probe_during_version_cleanup_starts_no_initialize cancels a probe during version cleanup, then checks that no owned group survives and only the version spawn succeeded.
Tests also check the shared engine/environment gate and registry, the ChildEnv spawn boundary, recheck_after_confirm_quit_launches_no_provider and escape_after_confirm_quit_keeps_sheet.
shutdown_between_actor_check_and_decide_commits_then_holds_turn pauses after the admission read and checks both orders of closing, persisted pre-quit commitment, no prompt and the recovery hold.
Socket-bearing test data folders use an explicit short path under /tmp, independent of macOS's default TMPDIR length.
The two database-lock quit tests hold BEGIN IMMEDIATE while an admitted settings write waits, then check shutdown within 10 ms, native quit within 150 ms, persistence and post-quit refusal.
launch_past_final_check_at_quit_is_registered_and_signalled pauses inside the launch gate after its final closing check and races native quit, then checks registration in the signal pass, SIGTERM exit and subsequent launch refusal.
A launch already past that check completes before closing; launches that reach the check after closing are refused.
launch_in_flight_at_quit_is_signalled_or_refused races one launch loop against closing over 20 runs and checks registration before the quit signal pass.
dropping_spawn_result_reaps_and_unregisters_its_group checks that dropping the returned guard reaps its owned process and removes its registry entry.
native_quit_signals_registered_provider_groups checks actual SIGTERM delivery through Engine::on_app_quit and subsequent launch refusal.
shutdown_after_admission_before_stdin_write_sends_no_prompt checks prompt refusal at the writer with an uncancelled delivery control.
quit_between_decision_and_dispatch_still_ends_promptly checks that a Starting thread with no session parks without waiting for the shutdown deadline, including when the shutdown write fails.
Test-only LaunchGuard timing records the longest held interval, excluding lock acquisition, without adding release instrumentation.
Request, grant and awake tests check request expiry, exact matching, thread isolation, needs_awake and retained-guard flags.
They do not inspect caffeinate liveness or the OS assertion lifetime.
Reaper tests check live parents, non-agent argv, the lazy executable/environment-read boundary and stored/swept versioned binaries reached through a provider symlink.
The versioned-binary test proves that a marked orphan started by its parent harness survives the child test's sweep.
record_redaction covers token removal, private captures, version/account sanitization and a fixture secret-shape sweep.
Tests guard failure redaction, launch arguments, terminal outcomes, snapshot ordering, metadata delivery, wait exits, driver-name and tool-name coverage.
Application tests enable gpui test-support to exercise Shell selection, its event pump, typed dock text and focus, chords and empty-Send Stop through the keymap in a synthetic window.
Dock regressions reject an old Allow once render, stale approval ids and an already-answered question index, with current-request controls.
A blocked real-session writer completes Provider B after dropping A's queued interrupt; a duplex-writer test checks the target again after a pending first-byte write.
Delayed actor Stop tests leave the later turn Completed and create no hold or drain, and synthetic-window tests check rendered Stop identity, Ctrl+C dispatch and a mouse release across a turn-changing repaint.
Completed-prompt and completed-Provider lifecycle tests reject late queued and started frames and feed fresh command ids through the codec and actor to prove new Provider work still opens.
An engine Refused-admission test checks busy is published with no Running row or pending_turn.
stop_target_uses_running_or_pending_turn checks the pure window rule on Running, pending-admission and busy Refused snapshots, including a Running row from another thread.
A synthetic-window test hides Stop and refuses empty Send without a target, and empty_send_stops_while_provider_is_checking checks empty Command+Return dispatches Stop with the running turn before readiness.
Admission tests cover Stop, Interrupt, Configure, SwitchView, Quit, keep-awake, idle parking and refusal waiting for a CLI end.
A composer unit test checks the preview cut notice; a synthetic window checks that long approval text scrolls within a bounded area while decision buttons stay on-screen.
The approvals-questions screenshots show the real-window scroll area.
Codec and storage tests cover all tool arguments, symlinks with missing parents, unverified targets, truncated Unicode previews, legacy approval rows and undecodable control failures.
Shell tests inject an isolated provider environment and admit no process to their startup sweep.
Their current-thread runtimes do not drive readiness tasks before replacing the probes, so their startup probes start no provider check.
recheck_after_confirm_quit_launches_no_provider deliberately checks an owned stand-in as its positive control, without running a real provider.
Clippy with -D warnings and cargo fmt --check check source quality.
The verify-veyra pages cover the real window, launch and relaunch row preservation, readiness without threads, streaming and background status, failure and Retry, default-mode persistence, header-branch clearing and cleanup.
App-launch's release check proves that release has no driver, using string scans with a debug positive control, ctl refusal and socket checks.
Its macOS sandbox allows only the release Veyra executable and denies every child executable, including installed-provider probes; an unavailable sandbox, invalid profile or missing exec-denial receipt fails the check.
Approvals-questions, stop-interrupt and restart-recovery cover the dock, queue controls, banners, keep-awake, quit sheet and startup recovery.
The multi-agent page covers family controls, stored task guidance, delivery, single-child archive holds, archive choices, quiet titles and the visible/open timing probes.
The Codex sections of new-thread, chat-send-stream, approvals-questions and multi-agent cover CLI model/effort readiness, streamed text, approval and question docks, notices, foreign-thread filtering, the root relay and the Claude-root Codex child card.
Permission-matrix records separately authorized live permission checks.
The ignored live tests require fresh human consent; ordinary workspace tests use only explicit stand-ins or missing provider paths in debug and release.
live_claude_approval_round_trip uses the public engine in Ask first to check an exact Bash request, Deny without a file, and AllowOnce on a repeat with the file created.
Actor tests check account-error retirement before queued work; a separate probe fixture checks Signed out while the no-account fixture remains Ready.
Live CLI behavior and performance need their authorized lanes; fake replay and structural checks cannot establish them.
Real provider permission behavior, native keyboard/IME/pointer delivery and Open Terminal's OS side effect are not proved by fake replay or the debug driver.
Composer Stop sends Interrupt and sidebar StopThread sends StopThread; real-provider continuation requires its authorized live lane.

Codex replay tests cover approvals, questions, elicitation, request cancellation order, resume errors and retries, all mode triples, early completion, foreign-thread filtering, notices and both command-output sources.
The mode-change replay sends Configure through the engine and completes one child conversation across four spawns.
The root proof uses the production veyra mcp relay, socket and peer checks, and the cross-provider proof settles a Codex child report into a Claude parent wake.
Peer tests cover omitted and included version fields, out-of-order replies, EOF tracking cleanup and the fatal line cap.
Relay tests hold the socket open while stdin or the parent ends and independently block socket and stdout writes before closing stdin.
The MCP half-close and queue-overflow tests hold an actor reply pending and require credential revocation and connection closure.
Codex live tests are ignored by default.
