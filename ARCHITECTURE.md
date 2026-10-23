# Veyra architecture

## Executive summary

Veyra is one native macOS chat window backed by an orchestration actor and per-thread Claude stream-json, Codex app-server or Grok ACP processes, with engine-owned PTYs for the human's shell drawer.
SQLite rows are the source of truth for projects, threads, turns, transcript items, tasks, the default permission mode and the window background choice.
The engine actor is the sole database writer after startup.
The gpui `Shell` owns window state and a read-only event projection, not another database connection.
The engine never depends on gpui.

Claude, Codex and Grok engine sessions, readiness probes, replay fixtures and debug recording are implemented.
Grok readiness reads initialize for its bare version, sign-in signal and per-model effort catalog, with Default-only menus when no catalog is advertised.
Claude, Codex and Grok server approvals and questions reach the same window dock, whose actions send typed decisions through the engine.
Codex async agent-message questions instead become persistent transcript cards whose option actions send ordinary user messages.
Human decisions and exact thread grants are persisted before their provider controls run.
Codec cancellation on provider completion, and cancellation on storage-blocked or blocked-shutdown paths, can precede stored request expiry.
Startup reaps marked orphan CLI groups, and the actor owns keep-awake, commit counts and actor timing.
The window owns render and event-batch counters.
Inactive-window family changes post system banners, and busy Quit opens a sheet.
Chat is the only agent surface.
Each thread has a separate in-memory shell drawer; the engine owns its PTYs and emulators, and ui::drawer owns its tabs and focus.
Engine startup binds the MCP socket server, and root Claude, Codex and Grok sessions receive its five actor-owned tools through the relay.

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
Shell -> Engine::open_shell -> veyra-pty worker -> login shell in own PTY session (portable-pty)
veyra-pty-reader -> alacritty emulator -> queued PTY writer replies
TerminalHandle -> queued PTY writer / emulator lock / exit status (no actor)
Shell -> runtime readiness probes -> short-lived probe CLI process (no turn)
Shell -> runtime branch task -> git child
Engine startup -> validated orphan sweep -> veyra-reaper thread
Engine actor -> owned caffeinate idle-sleep assertion

root CLI -> veyra mcp direct child -> DATA/veyra.sock -> peer + token checks -> Engine actor
Claude session -> private run/THREAD/mcp.json -> Claude root CLI
Codex session -> thread/start or thread/resume config -> Codex root CLI
Grok session -> session/new, session/resume or session/load mcpServers -> Grok root CLI

debug ctl -> private Unix socket -> socket thread -> gpui window actions
                                         debug session -> recorder / fake CLI
```

### Dependency hierarchy

```text
veyra -> veyra-engine -> rusqlite (bundled) + tokio
                     -> serde + serde_json + uuid + thiserror + tracing + regex + base64 + sha2
                     -> portable-pty + alacritty_terminal
                     -> platform -> libc + objc2 + objc2-app-kit + Rust standard library
veyra -> gpui-kit + gpui-pre
      -> tokio + serde + serde_json + tracing + tracing-subscriber + anyhow
```

Direct versions are exact pins and `Cargo.lock` is kept.
The engine's tokio features are `sync`, `rt-multi-thread`, `time`, `macros`, `process`, `io-util` and `net`; engine tests add `test-util`.
Only the application crate declares gpui dependencies.
It pins gpui-kit to =0.7.3 and gpui-pre to =0.3.9.
The engine pins the macOS-only objc2 =0.6.5 and objc2-app-kit =0.3.2 dependencies to read NSWorkspace display options.
The terminal window uses the alacritty_terminal cell, mode, colour and FairMutex types and portable-pty size type re-exported by the engine, so changes to those APIs also affect ui/terminal_view.rs.
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

`Engine::start` creates the production provider environment without waiting for its login shell, then locks the folder, starts the shared PATH loader thread, opens `veyra.db` through a private 0600 file, migrates, schedules PATH-gated orphan reaping, reconciles stored work and consumes the storage recovery receipt, removes stale run configs, binds veyra.sock, then starts the actor on a dedicated `std::thread`.
The actor keeps the lock file alive.
`Engine::start_with_env` accepts a prepared environment for isolated tests; the handle retains its resolution policy without owning another MCP server reference.
Ordinary unit-test `ChildEnv::load` calls and explicit isolated integration/window test environments never search for installed provider binaries.
`ChildEnv::isolated` starts with no provider paths, and `with_binary` adds only an explicit fake or missing path in either build profile.
Ignored live tests use the production loader.
The production loader keeps its login-shell PATH, debug overrides and unrestricted release reaper behavior.
The login-shell environment read runs on one dedicated thread in its own process group, after the instance lock, without delaying the window or its first-frame ready line.
One bounded shell command returns PATH and __MISE_DIFF separated by a NUL byte.
ChildEnv retains a non-empty shell __MISE_DIFF beside that PATH; when the shell omits it, the production child environment still inherits any application __MISE_DIFF.
`platform::login_shell_path` uses a bounded helper whose two-second deadline covers shell exit and stdout EOF.
Cleanup kills only the shell's group; an escaped descendant keeping the pipe open cannot extend the deadline.
`ChildEnv` logs the error and falls back to inherited PATH and __MISE_DIFF together.
`veyra.lock` is mode 0600 and its exclusive-lock retry lasts four seconds.
Only the lock holder stamps its pid or removes a stale debug socket.
A second launch refuses the folder and exits 1.
The OS releases the lock on exit or crash.
The scheduled reaper first awaits the shared PATH cell on the runtime.
Then `process::start` resolves canonical provider binaries through the engine's ChildEnv before validating stored pid/group pairs; this wait never delays the window or reconciliation.
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
`build.rs` embeds Git HEAD, or `unknown` outside Git, and watches HEAD, its reflog and the current loose ref.

## Quit and shutdown

Quit opens a sheet for running, pending- or refused-admission, Starting and background-busy threads and for drawer tabs with foreground or child processes.
Bare prompts do not require confirmation.
Thread lines include the project and child lineage; shell lines name the thread, tab and processes.
CancelQuit dismisses it and restores the originating composer or drawer focus until ConfirmQuit begins; afterward Escape and CancelQuit keep the sheet open.
ConfirmQuit synchronously closes the engine and cancels readiness probes and the branch task, then awaits Engine::shutdown_wait in a gpui task and calls cx.quit.
The debug ctl quit verb dispatches ConfirmQuit directly.
`Engine::shutdown_wait` refuses new commands, ends running and pending-admission turns, expires pending requests and asks every session to shut down.
It also waits for registered shell handles to finish, concurrently with the actor wait; shell workers use their one-second quit grace and the overall wait remains bounded by three seconds.
Shutdown persists Restarted holds for running, queued, Starting and background-busy threads in its own transaction, without replacing an earlier hold.
The engine owns one LaunchGate shared with the actor, every CLI session, ChildEnv and Probes.
It combines an atomic closing state with a small synchronous lock; no guard crosses an await.
Engine::shutdown sets closing while holding the gate before enqueueing Shutdown, asynchronously sending that message if the mailbox is full.
Dropping the last Engine handle and Actor::shutdown also set closing under the gate.
The Shell reads Engine::is_closing rather than keeping its own flag, and begin_quitting closes the engine and cancels probes and branch work for both quit paths.
The actor never holds the launch gate across decisions, SQLite writes, installation or effect dispatch, so a structured session task or PTY worker can enter the launch boundary without re-entering a held lock.
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
`Engine::on_app_quit` closes the engine before sending SIGHUP to registered PTY groups and SIGTERM to other registered provider/probe groups, then waits 100 ms on the runtime.
The application registers on_app_quit with gpui and runs begin_quitting before awaiting native termination cleanup.
macOS termination does not promise Rust destructors, so verification explicitly cleans run-owned provider groups.

### Quit commitment points

- Actor admission: the atomic closing read at the start of a message is its commitment point.
A command or provider event past a read of open commits as pre-quit work; a read of closing refuses new work and keeps earlier queues.
Shutdown ends and holds an admitted turn for recovery, while the prompt writer refuses a prompt whose own closing check has not passed.
Storage retry uses Tick's admission read before writing and installing retained changes, and each deferred message re-enters admission.
Actor::exited reads the closing state directly before queued-turn starts.
Effects run with no launch gate held and refuse starts while closing.
- Launch with registration: ChildEnv::spawn holds the gate through its closing check, OS spawn and group registration, so quit either refuses the launch or sees its group in the signal pass.
- Prompt write: the stdin writer uses an Acquire read of the atomic closing state immediately before each user frame and drops that frame if closing won, while preserving cancel replies before interrupt.
The prompt check is its commitment point; a write already past it belongs to a turn admitted before quit and shutdown interrupts that turn by protocol.
The writer takes no gate and an already committed write can finish after closing.

## Actor decisions, storage and recovery

`Actor` owns the SQLite connection, projection rows, live-thread map, item-key map, accepted-turn id set, text buffers, pending CLI versions and per-thread event counters.
It receives typed messages through a 256-slot tokio channel.
Engine::send returns a future that awaits mailbox space and then the command acknowledgement.
The window awaits that future in its gpui task; tokio's FIFO permit queue preserves the order of waiting sends.
Subscription and snapshot requests still use try_send and close their reply channels when the mailbox is full.
A timer sends Tick every five seconds; text uses separate flush and write messages.
The actor never waits for provider I/O.
The actor trims stored tool output to its last 10,000 characters.
The actor publishes window thread wait reasons, busy flags and pending-admission turn ids after reconciliation, then updates only threads whose thread, turn, request or live rows changed.
ChangeEvent boxes its large ThreadCreated and RequestOpened/RequestClosed row payloads to keep UiEvent compact; the window stores their unchanged row values.

Each Engine::subscribe creates an unbounded UI-event channel, so events accumulate without capacity backpressure while its window consumer stalls.
Provider stdin uses an unbounded Outbound channel whose writer drains frames into the child pipe; frames accumulate without a capacity limit while that writer stalls.
While storage is blocked, the actor appends provider messages to an unbounded VecDeque and retries them in order after storage recovers, retaining the remainder if storage blocks again.
These queues have no application capacity bound.
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
Version-only Ready metadata waits in Actor::pending_versions for the next transaction that already changes a row; Actor::write adds the version to that same transaction and clears it only after commit.
The existing two-second TextWrite timer also flushes pending versions if no other write arrives.
Successful writes install the version and publish ThreadUpdated after commit; a failed write retains it for retry.
Storage-blocked shutdown keeps its existing unpublished staging of retained changes.
The live pid/group UPDATE affects only rows whose stored pair differs.
Actor::write rolls back a transaction with no affected rows rather than committing or counting it; runtime changes and events still install.
Row-bearing events resolve from the final changed rows rather than being patched individually.
StartTurn effects name the turn until install builds its text from committed items and dispatches its control.

The send-to-reply path is ordered as follows.

1. CreateThread stores the thread and user item with a queued turn.
2. The Spawn effect starts the session on its own process group.
While the thread is still Starting, an exit before Ready fails the oldest queued turn.
A session that started adds a Stopped hold only if another turn remains queued, preserving any earlier hold; without queued work it adds no hold.
An asynchronous binary-resolution failure reports ResolutionFailed and retains the Stopped hold while Starting; provider crashes and sign-out follow the ordinary Exited rule.
Spawner::spawn returns a SessionHandle immediately; there is no synchronous spawn-error result.
3. Claude publishes Ready on a matched initialize reply or system/init; Codex publishes it on a successful thread/start or thread/resume reply, with the returned thread id and cliVersion, after any not-found fallback or active-writer retry.
Grok publishes Ready on a successful session/new, session/resume or session/load reply with its native session id and initialize version.
Ready changes Starting to Structured.
4. start_changed makes the queued turn Running, records pending admission and dispatches StartTurn with committed prompt text.
The first prompt waits for Ready.
The codec admits StartTurn only while its provider work is idle; Grok requires its native prompt map to be empty, and Codex requires TurnBinding::Idle.
The session retains the admitted prompt frames while it reports Dispatched to the actor.
The actor validates that the turn belongs to the current thread and epoch, then returns DispatchCommitted; only that acknowledgement releases the matching held prompt frames to the writer.
Interrupt and InterruptActive release the matching held prompt before its interrupt frames; Shutdown leaves the held prompt unsent and interrupts only the codec's active work.
The session reports StartTurnRefused when the codec refuses admission.
Grok announces any still-open foreign prompts as Provider work immediately after its Veyra turn ends, so retained native work always has a running actor row and Stop target.
ProviderTurnStarted opens the CLI's own turn under its reported id and restores a pending Veyra turn to Queued with its original ordinal.
The queue waits with TurnRunning while that Provider turn runs, an admission remains pending, or a refused start waits for the CLI turn to end.
Pending or Refused admission counts as busy for StopThread, Archive, Configure, idle parking, keep-awake and Quit.
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
The window, sidebar, header, dump and ctl wait consume that status, including background work and NeedsYou.
Spawn increments the epoch; stale session events are ignored.
Turn events must also match the running turn, unless uncorrelated content joins the Provider-origin row opened by ProviderTurnStarted.
An admitted `TurnEnded { park: true }` first sends Shutdown and enters `Stopping`, even if its turn already ended.
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

`store::MIGRATIONS` owns schema 4, with WAL, `synchronous=NORMAL`, foreign keys and a two-second busy timeout.
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
Migration 4 adds settings.window_background with a Translucent default and a CHECK allowing only translucent or solid.
SetWindowBackground and SetDefaultMode preserve the other setting and publish the complete SettingsChanged value only after the transaction commits.
Loading rows is read-only and treats any remaining empty CLI version as unknown.
Empty versions become unknown at the codec, probe and row-load boundaries; the actor and writes trust that input.
`store::apply` writes both live pid and group on Ready, clears both when parked, and preserves the first archive time on later archived writes.
Derived statuses and live state are not persisted as an event log.
Startup validates stored live pid/group pairs before reaping; reconciliation itself does not blindly signal stored ids.

Lineage CHECKs and the parent/root foreign key refuse grandchildren and inconsistent roots.
The hold CHECK allows only the typed hold reasons or null.
The singleton settings row starts in Full access and Translucent and constrains the three modes and two background choices.
Thread configuration remains in `config_json`; a default-mode change never rewrites existing threads.

`reconcile::decide` runs once before commands are accepted.
The actor routes that change through `prepare_commit`, completing interrupted text and unfinished tools in the same startup transaction.
It interrupts stored running turns, expires requests and holds affected queues as Restarted without replacing an earlier hold.
It replaces an already-started unaccepted wake, preserves an unstarted wake, and leaves running child tasks open.
Stored terminal-view columns are ignored during load and reconciliation, so an otherwise idle legacy thread opens parked in chat without a view-specific hold.
Stored TerminalSpan items remain readable.
Startup schedules the PATH-gated reaper with captured process pairs.
Independently, reconciliation writes every thread Parked and clears both live_pid and live_pgid; it does not await the reaper.
No provider work restarts automatically and Veyra writes no user CLI configuration.

## Shared provider process boundary

`provider/mod.rs` exposes typed controls and normalized events to the actor, not Claude wire types.
The actor dispatches through `Spawner::spawn`.
Its `Cli` variant sends Claude, Codex and Grok adapters to `session::spawn_provider` with the Engine's shared `ChildEnv`.
Scripted sessions exist only in engine tests and start no OS process.

`ChildEnv` shares one asynchronously filled login-shell PATH and optional non-empty __MISE_DIFF cell, with both inherited values as fallback after a failed shell or loader-thread creation.
A successful shell with no __MISE_DIFF retains the application-inherited value in the cell.
Explicit isolated environments strip inherited __MISE_DIFF, CLAUDE_CODE_AUTO_CONNECT_IDE and ENABLE_CLAUDEAI_MCP_SERVERS unless the caller supplies them explicitly.
LoginPathState is Pending, Loaded(LoginPath) or Fallback(LoginPath).
One private reader supplies environment construction and binary resolution from the cell; waiting only awaits readiness.
An isolated environment with no loader reports pending.
The loader fills one paired value once; later waits reuse it, including waits that begin after the fill.
Engine::start starts the loader immediately after the instance lock.
Session tasks, all three probe tasks and startup reaping await the cell before resolving a binary, so neither the actor nor the main thread waits for the shell.
Check again and Retry use the same wait.
The picker stays Checking until a probe answers, a session awaiting PATH stays Starting and honours pending Stop, shutdown and the shared closing gate, and the debug dump's login_path reports pending, loaded or fallback.
Resolution tries absolute entries from inherited PATH, that snapshot, then `KNOWN_BIN_DIRS`, returning a canonical absolute executable path.
Relative and empty search entries are ignored.
Known home-relative folders include .grok/bin, so a Finder launch can resolve Grok without that directory in either PATH.
Explicit isolated environments never fall through to home-relative install folders.
Executable checks and process-group setup for structured and probe children are platform functions.
Structured providers and probes have piped stdio and kill_on_drop(true); drawer shells use their PTYs and ChildGroup guards.
Structured sessions and probes create an owned process group and add VEYRA_PARENT_PID naming the application.
Drawer shells strip every VEYRA_* variable, including that marker, and are tracked by runtime handles instead of stored agent pids.
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
Claude additionally decodes subagent frames; Grok deserializes the initialization, permission, question and tool payloads it uses, and Codex deserializes the requests, notifications, items and replies it uses through tolerant wire types.
Unknown Codex methods and item kinds have serde fallback arms, and optional fields have defaults.
The bounded stderr tail is redacted before display and supplies failures when the CLI exits unexpectedly.
Claude Interrupt and Shutdown queue deny replies for every open approval and question with `interrupt: true`; Codex uses the per-kind cancel replies described below.
Decoded::take_outbound places every reply and ordinary control before any interrupt in the same batch, so decoder collection order cannot invert cancellation and interruption.
All three codecs use provider::StopLadder for the active turn's interrupted outcome and both deadlines: group SIGTERM after three seconds, then SIGKILL one second later.
Each codec owns its request cancellation and supplies an Interrupt frame carrying the target and its own protocol line.
SessionControl::Interrupt carries the turn id targeted by a human Stop and a typed StopReason.
Storage failure uses InterruptActive with StopReason::StorageBlocked to stop whichever codec turn is active, independently of actor admission.
Veyra-started turns use their stored turn id; CLI-started Provider turns open under the fresh id reported by ProviderTurnStarted before their first content event.
The actor drops unannounced Provider content instead of inventing an id the codec cannot match.
StopThread and Archive capture the running and pending-admission turn ids before ending their rows.
A targeted Interrupt whose named turn belongs to another thread or is no longer running or pending is a no-op, creating no hold, drain, control or outcome change.
Shutdown and startup recovery use StopReason::AppClosing for their shared turn-end reason.
The codec ignores an interrupt whose target is no longer active, including when a later Provider turn has opened.
A targeted interrupt cancels native requests and arms its ladder only when its target matches; active-turn interruptions resolve the target directly from the codec.
Claude and Codex cancel that turn's requests; Grok cancels every open request because session/cancel is session-wide.
The codec returns outbound Prompt, Message, Provider or Interrupt values, with each Interrupt carrying its target turn and supplying its own protocol JSON.
The stdin queue preserves that type, the writer applies its quit commitment check only to Prompt, and it checks an Interrupt's target against the codec's shared active id at each poll until its first byte is written.
An unsent interrupt for an ended target is discarded; its preceding cancel replies remain ordered before any matching interrupt.
Once writing begins, the writer finishes that frame before another frame.
A completed work group clears its ladder; Grok retains a session-wide Stop ladder while unfinished foreign prompts move into a Provider row.
An admitted new prompt starts with no active work or old ladder.
Human interrupts use "Stopped by you"; storage and shutdown carry their own reasons.
A session without a turn never arms that ladder.
SessionControls drains pending controls and closes its receiver on Shutdown or the shared LaunchGate closing state, retaining cancellation through process cleanup.
One drain_and_check_launch gates both initial launch and resume fallback, refusing closing, pending or consumed Shutdown, a closed control channel or a gone event consumer.
Before admitting StartTurn, SessionControls reads the gate's closing state and drains queued controls, so a pending Shutdown or channel closure prevents the prompt without changing cancel-reply or cleanup order.
Shutdown gives queued replies up to one second to drain, then aborts an unfinished writer before group cleanup.
An exit after requested Shutdown or parking is not a crash for any provider, including an idle provider that needs a group signal after stdin closes.
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
A result carrying api_error_status 403 instead ends Failed with its own redacted result text and no park, so another turn can use the process.
An active Stop retains precedence over that failure.
LifecycleState parses queued, started, completed and unknown values at the wire boundary.
A matching completed command without a preceding started command fails only that prompt, with the send-again message and no park.
A root start or resume appends tools::instructions through --append-system-prompt; children do not receive that addition.
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
SessionEvent::Notice(Notice) carries both ordinary notices and usage limits, with an optional reset instant in Unix seconds in Notice.reset_at.
Ordinary notices have reset_at None.
A missing reset_at field loads as None and is omitted on serialization, so older rows and ordinary notice bytes stay unchanged.
Only a resume-not-found stderr class starts a fresh native session; other errors retain the native id for Retry.

## Shell drawer ownership

The drawer is a utility for the human and never changes an agent's live state.
`Engine::open_shell(thread, cwd, size)` starts a tab and returns a TerminalHandle; `close_shell` and `close_shells` close one tab or every tab of a thread.
RuntimeState keeps the handles by thread, including closing tabs until they exit.
New opens prune finished handles and enforce terminal::SHELL_TAB_LIMIT, six non-closing tabs per thread; the window reads the same constant.
Both quit paths reach that registry even after the UI removes a tab or archives a thread.
No drawer command, actor event, transcript item or SQLite row is created.
StopThread affects only the agent.

`terminal.rs` owns each portable-pty child, master, writer, reader and alacritty emulator.
A tab worker starts without waiting for the provider PATH cell.
It starts `platform::user_shell()` with `-l` in the supplied thread folder; a missing folder refuses the launch.
The shell environment inherits the application's PATH rather than replacing it with the probe snapshot, because the login shell builds its own PATH.
It strips the four Claude nesting markers, every VEYRA_* variable, host-terminal identities and forced-colour settings, then sets TERM=xterm-256color and COLORTERM=truecolor.
The debug-only VEYRA_DEBUG_SHELL_FIXTURE instead selects the adjacent veyra-fake-cli in shell mode with one owned fixture.
The production shell receives no tool-server configuration, token or provider-resume arguments.
The shared launch gate covers the final cancellation check, OS spawn and group registration. portable-pty creates a session whose leader and group id equal the shell pid.

TerminalHandle exposes direct queued writes, resizes, subscriptions, shell status, pid and running process paths.
Exits and worker errors update the handle, never the actor.
ShellStatus distinguishes Starting, Running, Exited with a numeric code, Signalled, Closed and Failed with an error.
The pinned PTY library exposes signal outcomes only through its Display output; classification recognizes its signal prefix and never displays Rust debug formatting.
A worker unwind publishes Failed after its owned process cleanup.
Cancelled starts publish Closed.
Typed startup error paths supply the shell's start-failure words in terminal::run, and errors after startup keep their own runtime words.
The drawer displays that reason without parsing a provider or PTY library prefix.
A tab is running when the PTY foreground group differs from the shell's group, or the shell has a live child.
Background and stopped jobs therefore require consent too.
`platform::foreground_group` borrows a dup-owned PTY descriptor held under the terminal mutex through each tcgetpgrp call.
Dropping or reusing the writer's original descriptor cannot change that target.
The duplicate is released during worker cleanup.
`child_pids` uses proc_listchildpids and filters zombies.
Its return value is a pid count.
The window names processes through process_executable without reading or printing their arguments or environments.

Closing records the foreground group before sending SIGHUP to the shell group.
A surviving shell and the recorded foreground group receive SIGKILL after three seconds; app closing shortens that grace to one second.
The foreground deadline still applies when the shell exits before its foreground job.
One foreground grace-kill site checks that the recorded group still exists before sending SIGKILL and consumes that target once.
Observing the group gone clears the target, so a reused group number is never revisited.
The child guard signals its owned group before waiting on unwind and never signals it after a successful reap.
The reap and hangup paths hold the same pid mutex, and reaping clears both the terminal pid and group registration.
When both shell locks are needed, the pid mutex precedes the group registry.
Native quit copies the group registry and releases its lock before calling any terminal, so it never takes a pid mutex while holding that registry.
Reader and writer joins share the same bounded cleanup deadline; an escaped process retaining the PTY cannot keep the handle live indefinitely.
Such a process is outside the shell group, and a detached reader ends when it eventually releases the PTY.
A deadline warning can name its pid, never its arguments.
A Dock or logout quit sends registered shells SIGHUP before the engine's 100 ms native-termination wait; structured and probe groups receive SIGTERM.
The hangup flag prevents duplicate shell hangups across these paths.

The PTY writer queue is unbounded, so sustained input faster than the shell reads can retain queued bytes.
The reader parses at most 64 KiB under alacritty's FairMutex and releases the lock between reads.
Emulator callbacks use an Arc-shared Mutex list, making the emulator Send.
Every PtyWrite reply and colour query reply goes straight to the writer queue, independent of SQLite and actor admission.
The worker flushes an expired synchronized-update block without needing another input byte and coalesces dirty notifications on its 16 ms polling interval.
Kitty keyboard mode stays disabled.
Emulator clipboard events are discarded; clipboard access requires the human's explicit copy or paste action in the window.

`ui::drawer` owns each thread's open state, height, active index and numbered tab list.
These last only for the app session.
`ui::terminal_view` paints the active tab's current emulator grid, with bundled mono text and a still cursor.
Hidden tabs keep parsing while their subscriptions schedule no repaint.
The drawer opens under composition with the theme's 32 px tab strip, 134 px minimum, 236 px starting height and 72 px minimum tab width.
It stores the human's chosen height independently of the current space.
One displayed_drawer_height function clamps that choice against the header, the measured composition surface including its padding, and three reading lines.
Rendering, a drag's start and the debug dump read that function.
A smaller window or a longer draft temporarily lowers the displayed height, and the chosen height returns when space returns.
A drag starts from the displayed height and sets the chosen height on held-left movement.
Release, loss of the held button or a changed height budget cancels the drag.
Opening with an empty draft focuses the drawer; opening with a draft preserves composer focus, text and caret.
Hiding always returns to the composer.
Clicks change surface focus; output, exit and restart do not.
Tab keys exist only in VeyraTerminal.
Command-J remains a window binding; Command-T, Command-W and the tab chords win over terminal byte translation.
Closing the last tab hides the drawer and focuses the composer.
A running tab or an Archive that reaches running tabs opens a process-naming close sheet first; Cancel preserves the work and confirms close the chosen scope.
An ended tab keeps its screen and shows its numeric exit code, signal, closure or failure in words.
PTY-open and shell-spawn failures begin with "The shell could not start:" and a plain reason from the typed error mapping.
Failures after startup keep their runtime wording, such as "The terminal input stopped".
Synchronous open errors display their reason directly.
Return binds RestartShellTab only in its ended-terminal context and replaces its PTY and emulator in the same numbered tab, rechecking the folder before starting anything.
Live Return reaches the shell through terminal key translation without an action.
PasteShell, CopyShell and ScrollShellHistoryUp/Down are gpui actions bound in the shared VeyraTerminal key table.
Command-V reads the clipboard, strips ESC, DEL and all C0 controls except tab, newline and carriage return in both modes, and adds bracketed-paste markers only when the emulator requested them.
The wheel and Shift-PageUp/Down scroll history; the next input returns to the bottom.
A drag updates alacritty's own selection and Command-C copies selection_to_string.
IME is not supported.

Debug terminal grid sizing still accepts decimal columns 2–1000 and rows 1–1000 through VEYRA_DEBUG_TERMINAL_GRID, once at launch; invalid or absent values use layout sizing and release excludes the override.
Debug flood stamps count consumed bytes at FLOOD_START and FLOOD_DONE, excluding the markers.
The active view counts only paints between those stamps and reports payload bytes, elapsed time and repaint gaps through perf.flood.
The fake shell can echo input bytes, emit recorded raw output or start an owned sleep child for the background-job fixture.
It writes no Claude history and never reads the user's CLI homes.

## JSON-RPC peer and Codex codec

`jsonrpc::Peer<T>` owns the numeric request counter and pending caller tags, with a flag controlling the jsonrpc version field.
Replies return those tags directly.
Replies resolve by id, server requests retain string or numeric ids including zero, and Peer::receive returns replies, requests and notifications to its caller as Frame values.
Peer::eof clears its pending tags; ordinary field destruction releases them when the peer is dropped.
The session independently finalizes active work when stdout closes.
The shared writer in session.rs applies the launch-closing check to prompts and the active-turn check to interrupt frames before their first byte.
Codex and Grok use a reader task over the same cancellation-safe provider::lines::ByteLines reader as Claude.
session.rs reads Codex and Grok stdout through jsonrpc::Reader and Claude stdout through ByteLines directly; jsonrpc.rs imports only lines.rs and ProviderError, never the session.
The RPC reader queues four frames, an implementation choice that buffers a short reply burst while applying backpressure.
Four slots hold at most 16 MiB of capped wire bytes or 48 MiB of worst-case lossy UTF-8 text; the reader may additionally retain one decoded frame and its bounded byte buffer.
Reader::next returns text or an error, so an oversize line cannot leak into a caller's impossible branch.
A Codex or Grok line over 4 MiB fails the session with the cap in its crash text; invalid UTF-8 decodes lossily and non-JSON lines are skipped.
Grok uses the same peer and reader with the JSON-RPC version field enabled.

`codex::Codec` owns its Peer<Call>, app-server wire state, native thread and turn correlation, pending approvals and questions, item output, and protocol retry deadlines.
Conversation distinguishes opening with an optional resume retry from Ready with a native id.
TurnBinding distinguishes Idle, WaitingReply with an owner and optional provisional native id, and Bound with its owner and native id.
Each native turn maps to its owner and one Pending, Started or Ended state; binding describes admission and correlation, not another native lifecycle state.
InterruptRequest models an unsent interrupt, its first send, the wait for turn/started and the one retry.
next_deadline combines the shared ladder deadline with a resume retry without claiming every timer is an interrupt.
A session runs one codex app-server process, initializes with Veyra clientInfo without experimental opt-in and opts out of turn/diff/updated, then sends initialized and thread/start or thread/resume.
The readiness probe uses that same initialize shape.
Both thread calls carry cwd, the current model, approvalsReviewer user and the mode's approvalPolicy and sandbox; resume also carries excludeTurns true.
Root MCP config adds mcp_optional_startup_grace_ms 5000 and mcp_servers.veyra.tool_timeout_sec 30.
CodexOption owns both optional keys and their values.
An open rejection naming one key revokes that attempt credential, logs its key without the error payload, and retries once without that key.
Each key can be dropped once per root session; a repeated rejection fails, and children receive neither option.
Codex 0.162.1 accepted both keys in the retained live run.
The drop-key fallback is deliberate: a later CLI rejecting an optional key must not break every Codex root.
The user’s own permissionRequest hooks run inside Codex before a request reaches Veyra and can delay a dock; the retained run recorded about 65 seconds.
Whether tool_timeout_sec 30 includes the approval wait is not settled by that record; Veyra keeps the value at 30.
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
While a Veyra turn is bound, an own-thread request tagged with another non-ended native turn also docks under that Veyra turn for answers and expiry.
Each request keeps its native turn id separately from its displayed owner.
When the Veyra turn completes, the codec queues a cancel reply for each of its docked requests whose known native turn differs from the completed turn, before the actor expires those requests.
Requests for the completed native turn are removed without an extra reply.
An untagged request docked under that Veyra turn is also removed without an extra reply.
Before the turn/start reply, a request with a native turn id binds it provisionally; an MCP elicitation can omit turnId and still be answered by its request id.
A request for an idle own-thread turn opens Provider work even before turn/started.
Another thread's requests are refused without docking, and a start reply that changes a provisional id fails the session.
The matching turn/completed ends that turn even before turn/started, and a completed native id cannot reopen work.
Another native turn on this conversation can open a separately announced Provider turn; another thread id is filtered before turn routing.
Thread status, token usage, hooks, startup status and unknown notifications do not mark work or add notices.
A failed turn with the typed codexErrorInfo unauthorized uses Veyra’s sign-in instruction naming codex login.
Missing or unknown error kinds, including unfamiliar object shapes, retain the redacted provider message or the existing generic failure when no message exists.
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
The notice stores the selected window's reset timestamp, or the individual spend limit's timestamp for a spend warning, as optional Unix seconds beside the words before the time.
The transcript computes the remaining time on each render and applies the shared display projection to the notice.
A stored notice without a reset instant keeps its words and gains no time suffix.

Command and file approvals answer accept or decline; cancellation answers cancel before turn/interrupt.
Command approvals preserve the exact command and the typed cwd consequence.
The project-cwd consequence names the working folder.
A command approval with a known folder shows it in a separate labeled, muted quote; a reported outside cwd takes precedence over the project folder.
CommandCwd::NotVerified instead shows the unverified-location consequence and no folder quote.
The model-written reason is a typed ApprovalPreview hydrated at action construction or legacy deserialization and has its own labeled quote.
Command previews omit only the separately displayed command, cwd, reason or description and the named internal thread, turn, item, tool-call, timestamp, environment and request-kind keys.
Every other field remains in the marked preview, including sandbox options, and truncated previews keep their cut notice.
An incomplete JSON preview remains visible as stored text.
The working-folder quote stays beside the consequence above the scrolling details, with its path in a vertical scroll viewport of at most three lines; the reason scrolls with the command and preview.
ToolCall retains its serde-defaulted veyra_label flag for stored compatibility.
Transcript tool titles use unquoted bounded display text for both Veyra and provider names, followed by a distinct command, search target or other main argument when present.
Consent commands, previews, literal paths and approval tool names show Unicode controls, format and default-ignorable characters as visible ⟨U+XXXX⟩ markers.
Commands and Veyra's pretty JSON previews retain newlines; paths and tool names mark them.
Multiline command consequences name the line count, including blank lines, above the scrolling details.
The shell owns ApprovalScroll, pairing the docked request id with its details and folder scroll handles.
A changed request id, including closing the dock, replaces both handles; repainting the same request preserves its offset.
An always-visible kit scrollbar exposes actual overflow.
The thread-grant button shows the first line of the marked command, shortened to its available width; execution, native replies and stored grant identities retain the original bytes.
The UI takes ProviderKind for provider-specific behavior; display labels do not select behavior.
Only the exact project cwd may use the existing command grant; every other cwd requires a human decision.
Permissions approvals grant the requested profile for the turn, while Deny and Stop send an empty permissions object.
Questions retain provider question ids and send answers by those ids; cancellation sends an empty answers object.
AgentMessage delivery and question fields are typed separately from server requests.
A valid async questions list becomes ItemBody::AsyncQuestions with the original message text and each title, option list and optional selected index; SQLite's existing item JSON stores that additive shape without a migration.
The transcript renders additional agent message content above the cards with the same item-id markdown cache and shared reply_markdown helper as AssistantText, preserving its full text and paragraphs.
Empty messages and lines that repeat a trimmed question title or its bullet options are omitted, while additional message paragraphs remain.
Completed empty assistant items have no transcript row.
Each card quotes its provider title and option labels and explains that choosing sends a message, first stops active work and continues queued messages.
ChooseAsyncOption is a gpui action, bound to Command-1 through Command-9 and reachable through the debug driver; the engine validates its item, question and option identity and refuses a repeated choice.
One transaction persists the selected index and ordinary User turn.
If work is active, the choice uses the normal targeted Stop cancellation/interrupt path with a QuestionChoice reason, clears any hold and puts the answer ahead of already queued messages while preserving their order.
Turn ordinal updates are applied descending before that insertion to retain SQLite's per-thread uniqueness constraint.
The active session drains before the new prompt is sent; a choice deliberately clears any existing hold, including held queued work, and never requires Continue.
Card eligibility is a projection of item order: any later UserText item in the same thread closes every remaining choice, including after a typed answer.
The engine rejects choices on such cards, and the window disables their controls; no new stored closed flag or migration is needed.
An unnamed Command-1 through Command-9 action additionally requires a card in the thread's latest turn; explicit item actions retain their item/question/option identity.
The question stays in the transcript with Answered beside its quoted choice and disabled option buttons after selection.
An MCP elicitation is classified by codex_approval_kind first, then by a message naming serverName and a quoted or unquoted tool with an optional question mark.
It docks with the server and tool, offers no thread grant, and answers only action accept, decline or cancel without persisting provider grant metadata.
Other MCP forms are declined with a notice, and unsupported server requests get -32601.
A command row uses completed aggregatedOutput when non-null, otherwise the bounded outputDelta tail.
A no-active-turn interrupt refusal before turn/started waits for that start and sends one retry within the shared three-second protocol step.
Codex and Claude share process-group cleanup, the stop ladder, shutdown commitment checks and actor recovery paths.
launch chooses its provider setup once, launch_rpc matches the closed RpcProvider enum to choose Codex or Grok launch, codec and MCP wire, and launch_with shares fixture selection, recorder setup and the ChildEnv spawn boundary.
VersionRules carries each version probe's arguments, optional deadline, EOF rule and error text; the shared reader does not infer those rules from the provider.
Codec::profile matches every codec variant and assigns its direct-line or RPC reader and diagnostics.
The run loop chooses one SessionProfile from its codec for stdout reading, malformed-line diagnostics and resume-failure classification.
RootMcp holds either Claude's private Configuration and session credential or a shared McpConfig attempt-credential factory for Codex or Grok.
SessionIo owns the process and recording/credential lifetime, while Process owns the Claude, Codex or Grok codec variant.
Each Codex thread/start or thread/resume attempt registers a fresh one-use credential for the existing app-server pid, revoking the previous attempt before sending the next.
Resume refusals revoke immediately, including not-found fallback and every active-writer retry.
McpConfig retains the attempt hashes for inbound, stderr and recording redaction until the root session ends.
mcp.rs owns McpConfig and ThreadFrame, and its closed McpWire enum selects Codex or Grok when the config is built.
Codecs build the open frame without the credential; a Grok child keeps an empty mcpServers.
Immediately before the shared writer writes bytes, mcp::ThreadFrame writes Codex's dotted params.config keys or replaces a Grok root's params.mcpServers with the veyra entry, and its recording value carries only the token marker.

## Grok ACP codec

`grok::Codec` owns ACP JSON-RPC calls, prompt ownership, open requests, background task ids and its shared StopLadder.
SessionState names initialization, authentication, opening and readiness; its Ready state holds the native identity and optional displayed PromptWork, while OpenTarget names new, resume or load attempts without changing the stored SessionSpec.
PromptOrigin distinguishes each native prompt sent by Veyra from foreign prompts.
PromptWork is either one Veyra prompt with its turn id or one Provider turn that groups every open foreign prompt; native prompt tracking does not choose a different Stop target.
`grok::args` places --no-auto-update before agent and --no-leader before stdio.
--no-leader keeps the agent in the spawned process; without it, leader-enabled configuration can attach agent stdio to a shared leader over ~/.grok/leader.sock outside Veyra's group, which cleanup could not stop.
This flag's effect comes from Grok help and research and has not been observed on an installed Grok by these tests.
--no-auto-update avoids an update check before the first frame.
Ask first and Accept edits select default before agent; Full access uses agent --always-approve without --permission-mode.
Every Ask first and Accept edits session/new, session/resume and session/load carries _meta with yoloMode false and autoMode false.
After a successful open yields or retains the session id, the codec queues _x.ai/yolo_mode_changed with sessionId, yolo_mode false and auto_mode false before publishing Ready.
The runtime notification requests clearing settings-seeded auto; user allow rules remain in force.
Full access sends neither override and keeps --always-approve.
Grok follows the user's Claude and Grok permission settings, so Ask first asks only when they do not already allow the action.
Veyra adds no client hook gate.
Grok offers Ask first and Full access.
A stored Accept edits value remains stored but runs as Ask first; it grants no automatic edit response.
Model and effort are optional launch flags after agent, and Configure uses the existing respawn path.
No model or effort values are invented when no catalog has been read.
Initialize requests protocolVersion 1, disables filesystem and terminal client capabilities and declares _meta.clientType extension.
The initialization catalog remains an untyped Value; missing, null or mistyped metadata and catalog fields cannot prevent session authentication.
Only the probe reads model and effort fields, treating each invalid field as missing.
ChildEnv captures whether XAI_API_KEY is set at environment construction and passes that fact to the Grok codec; isolated environments report it unset and remove the key from their child environment.
An advertised xai.api_key method is chosen only with that captured presence; otherwise an advertised cached_token is used.
Authentication precedes opening the session when a usable method is advertised, and an authentication-required open permits one reauthentication and retry.
Authentication failures name Sign in with `grok login`; Veyra never launches that command.
A fresh conversation uses session/new with cwd and mcpServers.
A resumed conversation uses session/resume when agentCapabilities.sessionCapabilities.resume is advertised, otherwise session/load.
A load fallback discards every replay-marked update; no history is retained or imported.
Replay-marked notifications and notifications for another session are filtered before tagged-prompt acceptance, so they cannot confirm admission.
Only a recognized session-not-found error falls back to session/new with a notice; other errors preserve the stored native id.

Each session/prompt carries its Veyra turn id in _meta.promptId.
The first frame tagged with that id emits Accepted, or a matching completion emits it immediately before the end when no tagged frame arrived.
The matching prompt_complete notification or prompt RPC reply settles a Veyra-started turn once, whichever arrives first.
A Grok-started prompt also ends immediately on its own _x.ai/session_notification turn_completed update with prompt_id and stop_reason.
For a Veyra-started prompt, its own tagged turn_completed instead arms a two-second completion guard once.
A matching tagged prompt_complete or RPC reply can finish before that deadline; otherwise the session timer delivers the guarded outcome.
Untagged prompt_complete notifications are logged without payload and dropped, so an old tail cannot end a later prompt.
A prompt RPC error without acceptance fails without marking the prompt accepted.
If no JSON line arrives within sixty seconds of building the prompt, Veyra's chosen bound, the session fails: the turn ends Failed with that message, the Grok group stops and the thread parks until the next send respawns it.
Any parsed JSON frame clears that deadline, and there is no later silence timeout.
Unknown prompt ids on supported content updates or valid server requests are tracked as foreign prompts.
With no displayed work, the codec announces a Provider turn before emitting content or a request.
While a Veyra prompt runs, only foreign permission and question requests use its turn id, preserving the Veyra Stop target and the actor's persisted request dock.
Foreign text, reasoning, tool and plan updates still track the foreign prompt but emit no content during Veyra work; each is logged at debug with only its method.
Each prompt retains only the title and input of its dropped tool_call starts, keyed by native tool id, until the Provider turn emits ToolStarted or the prompt ends.
The first subsequent tool frame uses its nonempty title or the retained title, and the retained input when present, for ToolStarted; no dropped text, reasoning, plan or output is replayed.
Once foreign work runs as a Provider turn, its subsequent content is emitted there.
A prompt retains a ReplySegment with its number, open state and seen state; consecutive message chunks share a key, and tool, thought, plan or request boundaries close that segment before a later reply gets another key.
The first reply keeps its earlier key shape, so stored rows need no conversion.
Task reports select the last assistant text item, so dropped content cannot replace a child's own reply.
The human's answer uses the original native request id; if the displayed turn ends first, finish queues a native cancelled reply and the actor expires the request with that turn.
After the Veyra prompt ends, still-open foreign prompts immediately open one Provider turn in event order after its TurnEnded, without waiting for another frame.
That Provider row remains Running until every tracked foreign prompt has ended, so a second foreign prompt's completion cannot end the first prompt's row.
After a stopped Veyra turn ends, unfinished foreign prompts still open a Provider row, and the session-wide cancellation ladder stays armed until that work ends or cleanup stops the group.
A lone prompt_complete for an unknown prompt id is ignored.
A session-notification turn_completed records foreign or unknown prompt ids as ended immediately; an own prompt stays tracked until its completion guard or matching end settles it.
A settled prompt id cannot reopen on later tagged content.
Both x.ai and _x.ai task_backgrounded and task_completed spellings update the background task set.
Task updates prefer update.task_snapshot.task_id and fall back to update.task_id; the recorded backgrounding frames carry the root task_id.
The actor's existing background-work flag prevents Configure, parking and early task settlement while that set is nonempty.
Text, reasoning, tool rows and plans normalize to engine events; tool output emits cumulative-output suffix deltas and the actor bounds their retained tail.
A nonempty changed tool title emits ToolTitleChanged before output or completion.
The actor replaces the existing keyed ToolItem.call.name through ItemAdded, preserving its item id, sequence, input and output; it never creates a row for a title update.
Permission option kinds, session-update kinds, tool status and completion reasons decode into enums with an Unknown fallback.
Completion reasons also default on null, missing or other invalid values, so a matching completion still ends the prompt; only an undecodable completion scope is skipped.
Plan entries decode individually with a default status, and an entry without string content is skipped without discarding the other entries.
Unknown notification methods and session-update kinds are logged at debug with only their name and are skipped.
Ordinary session notifications without prompt_id are ignored without a malformed-notification log.
A notification whose scope, sessionUpdate, task, plan container or tool payload fails typed decoding is logged at debug with its method and skipped without its payload; it does not fail the session.
Notifications for another sessionId are dropped, and permission or question requests for another sessionId receive -32602.
Unsupported or malformed server requests receive JSON-RPC errors without approval.

Permission requests select options by allow_once or reject_once kind and never select allow_always.
A request with rawInput.variant Bash and only variant, command, description, is_background and timeout keys may use the existing exact-command thread grant, which still replies allow_once.
The grant uses the stable Bash tool key and CommandCwd::ProjectFolder; a Monitor variant or any other input shape gets no grant.
Every permission request in Accept edits follows the same dock path as Ask first, including in-project edits, protected files, mismatched targets and requests arriving after Stop.
The codec never selects an edit option automatically; typed answers and existing human-authorized exact-command grants keep their ordinary response path.
Permissions and questions without a prompt id use the running Provider turn when Grok started the work.
Other argument shapes offer no thread grant and show an unverified command cwd or a tool preview.
ProposedAction::consent is Once with its optional thread grant, or DenyOnly with Grok's lasting-grant reason when no allow_once option exists.
DenyOnly cannot carry a grant, and RequestKind::accepts owns allow decisions, exact thread grants and the Deny-only reason.
The window's request_consent derives enabled allows, reason and caption only from accepts; render, respond and dump read that projection.
RequestKind::accepts rejects both allow decisions for DenyOnly and preserves Deny, including after stored action round trips.
Existing stored approvals without consent deserialize as Once, and RequestKind moves an older outer grant_key into that consent.
An older DenyOnly action discards any outer grant_key when loaded.
The Grok new-thread hint says the CLI loads the user's hooks and MCP servers and follows their Claude and Grok permission settings.
It does not advertise an Accept edits choice.
The window can start Grok threads; without a catalog, each model and effort menu offers only Default, which clears the corresponding configuration value.
The model and effort action/key cycles return Default after the last advertised value; provider cycling still wraps to the first provider.
With the no-catalog fallback, a new Grok picker thread keeps both values absent, so its launch line carries neither -m nor --reasoning-effort.
Existing or tool-created threads can retain explicit values, which grok::args sends as launch flags.
A DenyOnly dock shows its lasting-grant reason, disables Allow once and Allow in this thread, and leaves Deny enabled.
Shell::respond checks RequestKind::accepts before sending, so disabled allow keys and explicit actions preserve the pending request and dock focus.
Both ask_user_question extension spellings decode flat sessionId, toolCallId, questions and mode params into RequestKind::Question under the running turn.
Questions need no question id or prompt id, and multiSelect may be null.
Replies are accepted outcomes with answers keyed by question text and arrays of selected labels; Stop replies with a cancelled outcome.
Stop first sends cancelled outcomes for every open permission request and question, then session/cancel with _meta.cancelTrigger ctrl_c.
The cancellation covers all open requests because the protocol cancels the whole session rather than one prompt.
The shared writer, target checks, ladder, cleanup and actor drain keep that ordering and prevent a stale Stop from targeting a later turn.

A root's session-open frame carries the Veyra stdio server in mcpServers with relay command, mcp argument and environment name/value entries.
McpConfig registers a fresh credential for each open attempt against the actual Grok process pid, revoking the earlier attempt.
Grok also revokes before its authentication retry and session-not-found fallback.
mcp::ThreadFrame retains McpToken until the writer serializes it and supplies only a token marker to recording.
No token-bearing config file, CLI argument or root CLI environment entry is created.
Grok starts a root's relay; the fake CLI starts it as a direct child inside the Grok group, so the group ladder or the relay's stdin EOF ends it.
The live Grok 1.0.60 recording shows its relay is a direct child in its own process group.
The ladder's signal to Grok's group does not reach that relay; the relay exits when Grok closes its stdin or dies, and the direct-parent check still refuses an intermediate process.
The same executable, direct-parent, epoch, live-turn and single-use socket checks apply to Grok roots.
Recorder redaction also masks credential values in ACP environment name/value arrays.

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
A later spawn or a Codex or Grok open attempt for the same thread revokes its earlier credential; a revoked open connection closes on its next input line.
Claude Session Drop removes its credential by hash and shared McpConfig Drop revokes its thread and epoch, and Actor::exited also revokes the thread and epoch before committing exit.
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
ProviderKind::ALL supplies the start_thread schema's provider choices.
list_providers reads the shared readiness catalog and offers adapters with Ready and a resolved path, their models and offered_modes, the caller's mode_ceiling and remaining fan-out.
The ceiling is the parent provider's effective run mode.
start_thread accepts each provider whose probe has a resolved path and Ready status, including Grok.
Probes populate that catalog and Engine::child_env shares it with the actor.
start_thread creates a child lineage, first task turn and UUID-v5 task keyed by caller and start_thread:client_request_id.
get_task reads only the caller's task and reports the child's Status::words with its settled summary when present.
message_thread creates a task on an owned child and reports started or queued with the derived wait reason.
stop_thread uses the existing child stop and settlement helpers with Stopped by its parent and leaves the parent unheld.
Identity is supplied by the authenticated connection, never by a self argument.
Ownership and a running-turn gate apply inside the actor before any tool decision commits.
A child may not start a thread, and the store's lineage constraints independently refuse grandchildren.
The derived permission order compares the requested child mode unchanged against the parent provider's effective run mode, with that parent mode as the omitted-mode default.
The child keeps the requested stored mode; its provider's run mode applies at launch.
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

The platform module owns macOS file modes, locks, sockets, login-shell PATH, executable checks, structured/probe group setup, group signals, process inspection, idle-sleep assertions, Terminal opening, user-shell selection, foreground/child queries and the fake shell's raw-input setup.
Only the fake CLI shell and its platform regression call raw_terminal_input; real shells own their terminal modes.
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
The initialize probe disables hooks through --settings, sets CLAUDE_CODE_AUTO_CONNECT_IDE=0 and ENABLE_CLAUDEAI_MCP_SERVERS=false, and adds --strict-mcp-config when no managed MCP policy file exists.
The production managed-policy path is owned by platform/macos.rs; explicit tests and debug replays inspect only their owned temporary policy path.
The probe adds no --mcp-config, and ordinary session launches retain the user’s hooks and MCP configuration.
A missing binary is Not installed.
Except for a missing binary or the captured signed-out signal, every probe failure or inconclusive reply reads Ready.
ClaudeAccount owns the typed account fields.
Signed out requires tokenSource exactly none, apiProvider absent or firstParty, apiKeySource absent or none, and no email, organization or subscriptionType field, including fields present as null.
API-key and third-party accounts remain Ready, and an unknown source cannot establish sign-out.
Missing or empty account remains unknown and Ready.
Both version and initialize reads reuse capped byte lines, and stderr is drained so a chatty probe cannot block its stdout reply.
Codex resolves its binary, initializes one app-server, sends initialized, account/read and model/list, and reads every advertised model page without starting a thread or turn.
One five-second timeout covers the whole Codex probe exchange, including every model page; cancellation and completion clean up through the shared process-group guard.
Its version comes from initialize.userAgent, model values from model/list, and effort levels from each model's supportedReasoningEfforts.
An explicit null account with requiresOpenaiAuth true reads Signed out; otherwise an absent account remains unknown and Ready.
A Codex probe failure other than NotInstalled follows the same unknown-and-Ready fallback as Claude.
Grok resolves its binary and starts the Ask first launch line with no model or effort flag in a fresh private empty probe directory.
One four-second initialize exchange reads its bare _meta.agentVersion, authMethods and _meta.modelState without authenticate, session/new or a turn.
An advertised cached_token, or xai.api_key with the captured XAI_API_KEY presence, reads Ready; otherwise it reads Signed out and the picker names grok login.
The catalog uses availableModels ids and names and each model's _meta.reasoningEfforts, with the advertised current model and each advertised default effort ordered first.
Null or mistyped catalog containers, models and fields are treated as missing without losing the authMethods sign-in reading.
The picker offers only the selected model's efforts, and a Grok model change clears an unsupported effort.
No modelState keeps the Default-only fallback; the probe does not run grok models.
Completion, cancellation and timeout stop the probe group and remove its private directory, including Grok versions that stay alive after stdin closes.
Debug fake probes execute the initialize exchange in probe-grok.jsonl rather than reading its header.
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
A Grok fake relay stays in the fake CLI's group and binds mcpServers through first-use matcher values in the expected session-open line.
The mcp_stop fixture record closes and reaps that owned relay before a later attempt starts another.
record_relay_pid stores only that owned relay pid under the same path restrictions, allowing process-exit assertions.
Capture-derived fixtures come from Veyra's own CLI recordings; other cases are Veyra's scripted wires.

The debug-only branches of `ChildEnv::load` are the only application read site for VEYRA_CLAUDE_BIN, VEYRA_CODEX_BIN, VEYRA_GROK_BIN, VEYRA_FAKE_FIXTURES, VEYRA_RECORD and VEYRA_REAP_PIDS.
The provider overrides replace their matching Claude, Codex or Grok binary.
Each Engine owns its recording sequence registry through its shared ChildEnv.
New threads register in creation order; each thread's spawn E uses tN-sE.jsonl, with probe.jsonl for Claude probes and probe-codex.jsonl for Codex probes; Grok probes replay probe-grok.jsonl.
Stored threads register when first spawned in that run.
Missing replay files fail spawn with their path; the child gets the absolute singular VEYRA_FAKE_FIXTURE only in debug builds.
The standalone fake executable reads that singular variable.

The debug Recorder creates private 0600 captures through platform::private_file, retrying a numbered free filename on a create_new collision without replacing another recording.
It binds the supplied cwd, optional home folder and known ids.
SessionIo sanitizes inbound and outbound recording values with its token hashes before calling Recorder::line, replacing any exact token with <id:token>.
The same hashes sanitize provider inbound values and stderr before UI, storage or crash text receives them.
The codecs hand the recorder typed native-id and version metadata decoded from their thread reply, system/init or session-open reply; recording protocol labels come from ProviderKind.
ChildEnv reads HOME once and supplies it to the recorder; `<home>` is a capture placeholder, not a matcher binding.
It replaces credential fields, redacts secret-shaped text, replaces hook output with a placeholder and keeps account and plan keys with masked values.
Typed ClaudeAccount fields preserve only tokenSource/apiKeySource none and apiProvider firstParty in masked account captures.
Outside account objects, a credential-named field whose name contains token or key keeps the literal none; every other credential value becomes <any>.
The fixture secret-shape sweep permits none only in token- or key-named fields, not Bearer credentials, passwords, auth, secrets or URL passwords.
Grok initialize agent and instance ids and hostname are masked, and availableCommands retains its shape with every value masked, including names.
The first initialize hostname is bound and replaced wherever it recurs, and email shapes are masked in text.
Account masking includes user-prefixed, email-containing, login, username, display_name, plan-prefixed and billing-prefixed keys.
Grok sessions/changed entries for another cwd retain their keys with masked values; the recorded project entry remains useful.
MCP server_status keeps server identity and status while masking detail values.
MCP servers_updated retains name, source and type while masking each server's URL, command, args, env and headers.
Every recorded http(s) URL loses its path and query.
Claude probes are recorded as probe.jsonl; a later probe replaces that capture only after the earlier probe has ended.
Codex and Grok probes are not recorded; probe-codex.jsonl and probe-grok.jsonl are replay fixtures only.
It records inbound/outbound wires and exit, updating Claude's header version from system/init.
Codex captures use the json-rpc protocol label, bind returned native thread ids distinctly and update their version header from thread reply metadata.
Grok captures use the json-rpc label, update their version from initialize agentInfo.version or the live _meta.agentVersion fallback, and bind the native id from the session-open reply.
An authMethods array is exempt from the credential-name rule in captures, secret-shape checks and the crash redactor; credential-named fields inside it are still masked.
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
ChangeSet::install publishes ThreadUpdated for a changed existing thread after the actor commits its rows, keeping the window's thread metadata synchronized.
AppState replaces that thread metadata without treating it as a new provider fixture lane.
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

The sidebar puts Add project and New thread in its top action group and Settings at its foot.
The header retains identity, status and the drawer button.
The sidebar reads projects in creation order, then each root and its children on a one-pixel guide, with the root glyph derived from family_status.
A thread row is one selection target with a fixed action space, a Stop control only while work can be stopped, and a menu revealed by pointer hover or keyboard focus.
A focused thread row uses ActivateThreadRow for Return and Space in the VeyraSidebarRow context, bound in ui/actions.rs.
Project and thread rows do not shrink inside the scroll column.
SidebarScroll retains the scroll handle and last revealed selection; a changed selection scrolls its immediate row into view during layout.
An unarchived child whose root is archived becomes a top-level row and keeps its own status.
OpenThreadMenu exposes Stop thread, Archive and, for a root with unarchived children, Archive, keep child threads.
A row menu closes after its own actions, family controls or a selection change, and quiet rows retain their siblings' title padding and Stop slot.
All resizing paths dispatch SetSidebarWidth.
The header reads title, status/elapsed time, project and a separate Git branch task on the tokio runtime.
The header's Command-J button toggles the selected thread's shell drawer; Chat remains the only agent surface.
TerminalView owns grid sizing, terminal input, selection, visibility, focus, wake coalescing and paint state, and uses TerminalHandle directly.
Prepaint derives cell width from the resolved bundled mono font and row height from the terminal type token, then queues a resize when the grid dimensions change.
Debug startup reads VEYRA_DEBUG_TERMINAL_GRID=COLSxROWS once into app state; valid decimal columns 2–1000 and rows 1–1000 override PTY/emulator sizing while preserving top-left painting and content-mask clipping, absent or invalid values use layout sizing, and release excludes the variable and override code.
The snapshot releases the emulator lock before merging each row's equal background colours and shaping ASCII runs with matching attributes.
Non-ASCII glyphs and combining marks start segments at their grid columns, wide spacers are skipped, and calt, liga and dlig are disabled.
The canvas paints those quads, shaped segments and a still cursor within its content mask.
The handle's watch channel schedules at most one pending next-frame notification, with no polling in the view.

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
ToolItem has optional approval: RequestId and interruption: ToolInterruption fields, defaulted and omitted when absent so older stored tool rows retain their serialized bytes.
TurnEvent::RequestOpened carries an optional tool: NativeItemKey within the provider session; the actor resolves that key to the individual tool item and persists its approval link.
The transcript reads the linked request's decision rather than matching commands across invocations.
A decision label requires the matching request row in the current window subscription.
Initial subscriptions and thread snapshots do not restore historical request rows, so a previously denied failed tool shows Failed after relaunch even when its stored approval link remains.
A legacy tool without an approval link shows its own Done, Working or Failed status.
The engine recognizes its older interrupted-tool marker without adding a stored field, and that marker shows Stopped when the owning turn is Interrupted.
ToolInterruption::TurnEnded represents an open tool closed when its turn ended, independently of the visible explanation.
Started N agents cards, singular Started 1 agent, group children after the parent turn that started them, including archived children whose Open is disabled.
Task::start_request exposes an engine-owned StartThreadRequest view over the compatible stored start_thread key.
The projection finds the parent turn from that typed client request id and the tool argument, falling back to the latest parent turn no later than the child's first task start.
Each row uses its child's own status glyph and words, SelectThread as Open, and StopThread as Stop only while the shared stop_target rule names a turn and the child is unarchived.
TaskCard rows precede the other items of their own turn and show Task from the parent provider.
Task and Wake turns label the parent title and distinct reporting children respectively, using the same row gap before their content.
A Delivered task labels only its named source assistant row Report delivered, including when queued tasks settle with a later task's reply; later identical user text cannot move the badge.
A task without a recorded source shows no delivery badge, including older stored tasks and a child stopped before producing an assistant item.
The row renderer and dump use the same report_delivered predicate over the task's stored link.
Turn::recency orders the transcript, ThreadView.last, failure cause and Retry by actual start, or end time for a turn that ended without starting, with ordinal for ties and queued turns after started ones; stored queue ordinals retain their admission role.
The actor attaches a session notice to a running turn when one exists, otherwise the latest started turn; queued and never-started turns cannot take a notice.
Startup storage-loss notices use the same rule on the reconciled rows, so an attempted-start turn recovered as Interrupted can take its notice because it keeps its attempted start time.
When no eligible turn exists, the shared notice selector logs the thread id and notice kind without the notice text.
ReplyLineBreaks applies to assistant reply rows and uses the kit's Markdown AST hook to turn prose soft breaks into rendered line breaks, preserving lists, emphasis, tables and code, with the projected Markdown retained for source copying before the line-break transformation.
The stored item retains its original text separately from this display projection.
AppState caches each thread's ordered item ids and row positions, rebuilding them on item insertion, turn metadata and snapshots.
It fills shared markdown source strings for assistant and reasoning rows before they enter the cached order, refreshing only the changed item and removing archived rows.
The row renderer receives that row's cached markdown source without rebuilding it.
The list reads items and turns directly from the projection instead of cloning the transcript on every render.
New family cards insert at their derived turn positions; text remeasurement and expansion translate cached item positions past every preceding family card.
AppState::text_rows selects only a text-only pump batch's changed rows in the open thread for remeasurement.
Other batches still remeasure the list for structural, outcome and queue-label changes.
The inline streaming caret remains part of the markdown source, so a growing assistant row still takes the kit's full-replacement parse path.
Failed tools show their last nonempty output line, truncated to one row at its width.
The window and dump share row_outcome, which annotates only each ended turn's final item.
The failure card follows the final item of its failed turn, whose outcome caption reads Failed because the card shows its cause.
Historical failed turns retain the cause in their caption.
OutcomeLine keeps words and optional elapsed seconds separate until rendering, while the debug dump formats them as one line.
Elapsed time renders in a separate outcome segment, and an outcome reconciled after Veyra closed has no elapsed segment.
A user scroll detaches following; new text, requests and tool rows preserve that state.
Scrolling within text.read.line_height of the bottom or choosing Latest reattaches.
Latest appears only while following is detached and content remains below.
Expanding a row remeasures it without moving its top anchor.
Streamed text and tool input remain unbounded outside the 4 MiB wire-line cap.
The failed view uses the latest nonqueued turn and places its cause, Retry and Open Terminal on the project folder after that turn's content.
Queued wait words sit directly below their own user bubble.
Each transcript item has paragraph spacing, and a stored TerminalSpan uses a bordered block, terminal text and the label This turn ran in the terminal.
It renders the provider’s failure cause and any next step in that cause without appending a generic sign-in footer.
Claude, Codex and Grok sign-in failures include their sign-in instruction in the cause, so it appears once; a 403’s own wording is never supplemented with login advice.
header::status_text supplies shared user-facing status words, including Needs you, for the header, family card and sidebar tooltip; Status::words retains dump spellings.
The header overrides them with Starting and the thread's own provider while the engine's published wait reason is Starting.
Retry probes again; once the thread's provider is Ready it clears any hold and resends the failed turn's own user item as a new turn.
A newer queued message remains queued and is never reused as the Retry prompt.
Send also clears a hold when the thread has no queued turn.

The composer uses TextareaState and provider/model/effort/permission chips.
Typed gpui action payloads reject malformed ids and enum values at dispatch, with a visible error.
On New thread, OpenProviderPicker and its key open the provider menu through a controlled kit popover.
An open menu rebuilds its readiness labels when probing begins and when each reply arrives.
Not-ready providers disable Send with their repair command, which stays below the composer when its provider menu opens.
A signed-out failure card shows the provider's one sign-in instruction from its cause.
Codex and Grok use the same picker and composer, with model labels and effort choices read from their probes.
The selected model chip uses the catalog display name and falls back to the stored value when the catalog lacks it.
Grok uses its initialize catalog, with Default-only model and effort menus when modelState is absent; switching providers clears both selections and resets the permission mode to the new provider’s run mode of the stored Settings default.
Codex's commands, file changes, permissions, MCP approvals, questions and retry/limit notices use the existing dock, question card and transcript rows.
A Claude root's Codex child uses the same task card, breadcrumb and family projection.
CreateThread sends the draft configuration; subsequent Send targets the selected thread.
On an open thread, model, effort and permission chips send Configure, which is refused while work runs and parks an idle session.
Enter submits, Shift+Enter inserts a newline, and the textarea propagates Command+Enter to the bound Send action without a second subscription send.
ProviderKind owns offered_modes, run_mode and next_mode.
Claude and Codex offer three modes; Grok offers Ask first and Full access.
The composer renders the effective run mode and cycles only offered modes while preserving unsupported values on an existing stored thread.
New thread resets text and applies the selected provider’s run mode to the current stored default.
Settings occupies the main column and opens through OpenSettings, the sidebar foot, the native menu or Command-comma.
Its three-mode default row and two-choice Window background row use actions and persist independently.
Command-Option-B cycles SetWindowBackground.
SetDefaultMode publishes SettingsChanged; Escape or thread/project selection leaves Settings.

`ui/actions.rs` owns actions, the native menu and one bindings table used by both key installation and shortcut_chip.
Ctrl+C is literal control in the composer and preserves its draft while interrupting the turn.
ToggleShellDrawer is bound to secondary-j without a key context, so it remains reachable from drawer focus.
gpui resolves key bindings before keydown listeners.
The one bindings table adds a predicate excluding VeyraTerminal to every non-drawer non-Command binding and to plain Command-Left and Command-Right, so future bare-key bindings cannot take terminal input.
Command-N remains a window action while the drawer has focus; drawer tab bindings use VeyraTerminal.
The pure terminal_key_bytes maps Shift-Enter to ESC CR, Option-Left and Option-Right to ESC b and ESC f, Command-Left and Command-Right to Ctrl-A and Ctrl-E, and Ctrl-L to form feed.
It also maps ordinary text, control bytes, navigation and function keys, including application-cursor arrows, directly to the PTY queue.
Selection, paste and scrollback belong to TerminalView; IME remains unsupported.
The approval dock (`ui/composer.rs`) renders only the codec's typed ProposedAction and does no disk work to render it.
The chat column bounds the composer by its available height; the transcript gives up space before the dock.
Flex layout measures the hold row, request count, consequence, wrapped decision buttons, focus hint, input and toolbar before shrinking the details to the remaining height, capped at composer_max_height (240px).
The command or file path, model-written reason and bounded argument preview share that details scroll area, which keeps at least one code-text line.
The consequence, any Deny-only consent reason and any outside working folder sit above it, and Allow and Deny stay below it.
The folder's viewport keeps at most three label-text lines.
Docks, the input and toolbar share one raised surface, rim and height budget.
The message input grows from 48px to 240px while space remains and keeps its draft scrollable under pressure.
The composition surface yields its scrolling details and input space before hiding decisions, while the transcript retains three reading lines.
A fade at the transcript's foot draws only over the opaque content plane.
Decision controls put Deny and Allow once before the optional thread grant, so a wide grant wraps after both decisions instead of forcing three rows.
The approval matrix asserts that both theme window sizes retain Deny-only controls, the long folder's three lines and at least one code line with all fixed controls present.
Folder, details and question text use one bounded-scroll helper whose caller supplies minimum and maximum heights, with independent kit scrollbars visible only for actual overflow.
Its 16px logical thumb minimum leaves travel on a 20px one-line track; the kit's 4px end insets leave an 8px painted thumb at that minimum.
The working-folder quote uses the 16px label line height and the scrolling dock question uses the 20px UI line height.
Other quotes inherit their parent line height and keep natural height; async card titles use the transcript row’s 24px read line height.
Question text scrolls from one 20px line up to the 240px composer cap beneath its fixed label; the combined question region keeps a 44px minimum for label, gap and one text line.
Up to four options use one column; larger lists use three columns.
Option captions wrap to two lines with an ellipsis inside buttons of at least 28px; overflowing lists scroll in a 28–240px bounded viewport with the same overflow-only scrollbar.
Shell owns QuestionScroll separately, keyed by the docked RequestId and question index; a changed target replaces both its text and option-list handles, and repainting the same target keeps their offsets.
Shell owns both approval handles and replaces them when the docked request id changes, so each new request opens at the top with its full marked path available by scrolling and no elision.
The stable folder element id does not own that reset.
The dock shows 1 of N for multiple pending requests and a separate Question 1 of N count within a multi-question request.
Approval chords and number keys answer only while the dock has focus; the composer keeps Command+Return as Send.
Consent buttons carry their displayed request id, and question choices also carry the displayed question index.
Shell::dock_target selects the request and question index for dock rendering, consent buttons, the debug dump and keyboard handlers.
Button ids contain the request and question identity, so a repaint to a different target cannot complete an earlier mouse press.
Keyboard handlers retain the focused dock's rendered target.
Respond rejects an expired or missing request, and a question choice rejects an index that has already advanced, without selecting another request.
Because a docked request implies a running turn, composer Command+Return with an empty draft dispatches Stop, which interrupts the turn and expires the request.
Enabled approval buttons show their answer chords only with dock focus; otherwise the dock offers Command+Shift+Y to focus it.
A disabled thread-grant button uses Allow in this thread because its consent carries no grant.
Eligible thread grants retain the command caption.
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
The Look global carries the stored background choice and macOS display flags.
The full-height sidebar is the glass plane; the header, transcript, composer, drawer and other main views stay on an opaque content plane.
Translucent uses gpui's Blurred window background and the sidebar's shell tint at coverage 0.96 in dark and 0.98 in light.
Solid uses Opaque and the opaque shell tint.
Theme::apply restores that window background on each appearance change instead of leaving the kit's background reset in force.
It also makes the kit root's background transparent in effective Translucent, so the kit does not cover the sidebar's glass tint, and restores an opaque root in Solid or forced Solid.
Content, raised card and floating colour roles form the dark elevation ladder; light uses white planes and shadows.
Floating sheets use the floating radius and shadow, and the kit's popup shadow is enabled.
The platform reads Reduce Transparency, Increase Contrast and Reduce Motion from NSWorkspace at launch and window activation.
Either transparency or contrast flag forces Solid without changing the stored choice, and Settings names the forcing flag.
While forced, its background row selects Solid and disables both choices, and the native Toggle Window Background menu action is unavailable.
SetWindowBackground and Command-Option-B leave the stored choice unchanged.
Clearing the flag restores that choice.
Increase Contrast raises borders to a component contrast colour and uses the text colour for muted roles.
Reduce Motion sets gpui's reduce-motion flag and snaps every motion token, including the spinner, to zero.
The working glyph remains still.
The one-second status clock repaints while work runs or the selected Chat transcript contains a notice whose reset instant is still in the future.
An expired reset or a hidden transcript no longer requests notice-clock repaints.
Theme values and roles live in ui/theme.rs.
Geist and Geist Mono are bundled from Vercel's official release with its OFL beside them.
Window geometry comes from the theme and can be overridden by the window manager.

`ui/notify.rs` derives banners only when an inactive window's family enters NeedsYou, Done or Failed.
Family status includes unarchived children, so pending background work prevents a Done banner.
The banner contains the family root title, derived by collapsing whitespace in its first user message and taking the first 80 characters, and a status word.
It excludes assistant, tool and later user text.
Its root-id tag lets a click activate the window and select the root or the first child needing attention.
`scripts/bundle-app.sh` builds a macOS app bundle with dev.veyra.app identity, copies Veyra.icns into Contents/Resources and names it in CFBundleIconFile.
It signs the bundle ad hoc because macOS refuses notification authorization to an unsigned bundle.
System permission and native banner clicks require operator verification.
The debug-only DebugNotificationResponse action passes a banner tag to the same notification_response handler as the native callback, allowing stale-tag routing checks without OS delivery.

Provider display text uses the engine's pure text::display_line projection, with its own 400-character input cap, a trailing ellipsis when cut, collapsed whitespace and removed Unicode format and default-ignorable characters.
Dock reasons, async card titles and options, and family/plan labels use that projection with quotes and the existing truncation mark; reasons have their own labeled, muted element.
Transcript tool titles use the same cleanup without quotes.
The shared display_text projection removes invisible format characters and draws at most three consecutive combining marks while retaining paragraph breaks.
Expanded tool input, notices and failed-tool previews use that projection without changing their stored text.
Consent projections retain the first three combining marks and show each additional mark as a visible ⟨U+XXXX⟩ marker, preserving every character.
Literal consent paths and approval tool names instead retain their full marked consent projection in both action text and labeled quotes; commands retain their marked text with heredoc newlines.
The debug dump exposes provider_text entries in approval docks and display_questions plus choices_active on async items beside the unchanged raw body.
Those fields use the same display projection as rendering, while raw options remain available for exact protocol assertions.
The debug tool_label and dock grant_caption fields share the row and grant-button rendering helpers.
The debug-only DebugScrollApproval action moves the actual details handle used by wheel scrolling; details_scroll exposes its offset and maximum only when the scroll state belongs to the currently docked request, without changing consent state.
Raw option strings remain in the stored card and are selected by validated indices for UserText, so display cleanup cannot change the sent message.
The answer bubble uses the card's cleaned one-line option text while storage and outbound protocol bytes remain exact.
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

Action accepts optional JSON data, keys parses the entire sequence before dispatching a key-down then key-up for each keystroke, text uses the focused textarea's public input handler or dispatches character key events to the focused terminal, and quit dispatches ConfirmQuit.
The keys_dispatches_down_then_up_per_keystroke unit test guards that order.
The native keyboard proof opens a composer chip through key release; terminal input handles key-down only, so adding key-up emits no extra terminal bytes.
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
Its fake checks isolate data and use only the fake CLI; explicitly authorized live checks record actual provider wires.
Proof stays outside disposable scratch, and driver and cleanup actions target only processes this run started.
The notification backdrop records its own pid, and shared driver helpers verify its executable against the scratch bundle before reactivation, after each open and immediately before every Cleanup signal.
A dead recorded backdrop fails reactivation rather than starting an unrecorded replacement.
Solid captures use a window capture and require the owned window to remain on-screen and frontmost.
Translucent captures use the window's exact on-screen rectangle over run-owned backdrop windows.
Immediately before and after each region capture, the driver checks frontmost identity, unchanged bounds, full coverage by one backdrop window on one display, and no intersecting on-screen window above Veyra.
A changed guard discards the file; the driver makes at most three attempts in total.
The Settings page plants a run-owned occluding window and requires rejection without a retained capture.
Release ignores the debug reap filter and can signal any validated orphan, so the skill keeps release checks away from another run's deliberate orphan.
Dump's awake reports whether the actor retains the guard, without checking the child or OS assertion after spawn.
Dump also reports each known live CLI pid with resident_bytes.
Debug perf reports commits, actor_max_ms, actor_busy_ms, mailbox_free, repaints, max_batch_events and the completed terminal flood measurement.
Dump drawer holds the selected thread's open state, height, active tab, tab processes and exits, plus its active terminal grid and paint count.
Dump focus reports composer, drawer or none from focus-handle observations.
A modal sheet reports none; it remembers the originating surface for Cancel. composer_caret reports the input offset.
The debug-only DebugFocusShellDrawer action targets the same terminal focus handle as a click, for deterministic drive preconditions.
All terminal repaint counters and flood measurement consumers are excluded from release builds with cfg(debug_assertions).
mailbox_free is the command sender's unreserved channel capacity; slots assigned to waiting sends count as used, so it reads 0 while any send waits.
actor_busy_ms is the elapsed time of the message currently being handled, or zero when idle.
Dump includes every request row the window has received, including its state, plus quit_sheet, window_active, notifications.posted and transcript.items with transcript.follow.
The composer exposes offered_modes and permission_picker_open beside its effective mode; selected_thread.config retains the stored configuration.
The composer dock adds action_text, consequence and preview from the same formatter the window renders.
Its allow_once_enabled, allow_in_thread_enabled and consent_reason use the same typed consent as the rendered controls.
DebugStallUi blocks the window thread for its supplied milliseconds so the pump can be measured under a controlled stall.
The debug-only veyra::DebugDisplayOptions action overrides the three display flags without changing macOS settings.
The look dump reports the stored choice, effective background, flags and forcing reason.
The debug-only `veyra::DebugAgeQuiet` action accepts a thread id and minutes, subtracts those minutes from that Working thread's quiet activity timestamp, and repaints; the next activity event replaces it with the real timestamp.
It changes no engine status or turn timing, and `header::quiet_for` remains the display rule.

## Differences from design-6

The runtime sections above own the implemented rules.
The earlier follow-ups on writable Git roots, root family instructions and Grok’s two offered modes are already reflected in design-6 and are not differences.
The following clarifications and remaining differences are checked against the code.

- Design 7.3’s Codex examples leave live output and error variants open.
  The codec prefers completed aggregatedOutput, accepts quoted tool names and a trailing question mark in MCP approval messages, and distinguishes resume-not-found, active-writer and no-active-turn refusals by text.
  Async-card choices stop active work before sending an ordinary user message; command thread grants require the exact project cwd (`provider/codex.rs`, `commands.rs`).
- Design 7.4’s Grok catalog recipe calls grok models beside --version when a catalog exists.
  The implemented probe instead reads authMethods, bare _meta.agentVersion and optional modelState from one initialize, without authenticating or opening a session (`provider/grok.rs`).
- Clarification of design 7.4: Grok’s question wire is flat and needs no question or prompt id.
  Session/resume replies can omit the session id, and session/load is the fallback when resume is not advertised.
  Restrictive sessions turn off settings-seeded auto/yolo, but preserve user allow rules and add no client hook gate.
  Exact-command grants accept only the recognized Bash input shape (`provider/grok.rs`).
- Design 7.1’s shell snapshot can supply no non-empty __MISE_DIFF.
  In that case ambient children retain any application-inherited value beside the shell PATH; failed shells retain both inherited values (`provider/env.rs`).
- Design 11’s captures use collision-safe numbered names and additionally mask account metadata, Grok identity/hostname, commands, server connection details, URL paths and queries, and email shapes (`record.rs`).
- Clarification of design 9: Provider-text presentation distinguishes marked literal consent text from one-line projected labels.
  Chosen async option messages retain raw bytes for storage and sending, and their user bubbles use the same cleaned text as the card (`text.rs`, `ui/provider_text.rs`, `ui/transcript.rs`).
- Clarification of design 9: Dock layout keeps decision buttons visible, bounds outside-folder text to three lines, scrolls long details and question text independently, and bounds option captions to two lines with an ellipsis.
  The input grows into the space its docks leave and remains scrollable under pressure (`ui/composer.rs`, `ui/provider_text.rs`, `ui/mod.rs`).
- Clarification of design 12: Ordinary tests use explicit isolated provider environments in debug and release; only ignored live tests resolve installed CLIs.
  Terminal throughput measurements must name their grid, which the debug launch override can fix (`provider/env.rs`, `ui/mod.rs`).

- Design 7.2 asks for retry without a refused Claude probe flag.
  That fallback is not implemented: session::probe spawns once with claude::probe_args.
  If a later CLI refuses --settings or --strict-mcp-config and exits without initialize, readiness is Ready with no models after the probe error (`provider/session.rs`, `provider/claude.rs`, `provider/mod.rs`).
  A live check on 2026-10-20 recorded Claude 2.1.304 accepting both flags.
- Design 7.2 also asks for retry without a refused --append-system-prompt.
  That fallback is not implemented: every Claude root receives the flag; refusal exits before initialize and fails the turn (`provider/claude.rs`, `provider/session.rs`).
  A live check on 2026-10-20 recorded Claude 2.1.304 accepting it.
- Design 9.1 asks for a separate next-step line.
  The failed view shows each failure's own cause, Retry and Open Terminal, without a separate next-step line.
  The cause carries the next step when the provider supplies one (`ui/transcript.rs`).

## Source map

- [Entry and runtime](crates/veyra/src/main.rs), [engine handle](crates/engine/src/lib.rs), [data folder](crates/engine/src/data_dir.rs).
- [Actor](crates/engine/src/actor.rs), [decisions](crates/engine/src/commands.rs), [domain/indexes](crates/engine/src/domain.rs), [live variants](crates/engine/src/live.rs).
- [Relay](crates/veyra/src/relay.rs), [MCP socket and config](crates/engine/src/mcp.rs), [tool decisions and guidance](crates/engine/src/tools.rs).
- [PTY and emulator](crates/engine/src/terminal.rs).
- [Drawer tabs, focus and close sheets](crates/veyra/src/ui/drawer.rs), [terminal canvas, input and repaint measurements](crates/veyra/src/ui/terminal_view.rs).
- [Storage/schema](crates/engine/src/store.rs), [recovery](crates/engine/src/reconcile.rs), [events/snapshots](crates/engine/src/events.rs).
- [Provider boundary](crates/engine/src/provider/mod.rs), [environment](crates/engine/src/provider/env.rs), [sessions/probes](crates/engine/src/provider/session.rs), [Claude codec](crates/engine/src/provider/claude.rs), [JSON-RPC peer](crates/engine/src/provider/jsonrpc.rs), [bounded lines](crates/engine/src/provider/lines.rs), [Codex wire types](crates/engine/src/provider/codex/wire.rs), [Codex codec](crates/engine/src/provider/codex.rs), [Grok codec](crates/engine/src/provider/grok.rs).
- [Fixture format](crates/engine/src/fixture.rs), [fake CLI](crates/engine/src/bin/veyra-fake-cli.rs), [recording/redaction](crates/engine/src/record.rs).
- [Window/pump/fence/branch](crates/veyra/src/ui/mod.rs), [transcript](crates/veyra/src/ui/transcript.rs), [composer](crates/veyra/src/ui/composer.rs), [Settings](crates/veyra/src/ui/settings.rs), [actions](crates/veyra/src/ui/actions.rs), [theme](crates/veyra/src/ui/theme.rs).
- [Banner decisions](crates/veyra/src/ui/notify.rs), [macOS app bundle](scripts/bundle-app.sh).
- [Process supervision](crates/engine/src/process.rs), [Platform mechanisms](crates/engine/src/platform/macos.rs), [selection](crates/engine/src/platform/mod.rs), [boundary guard](crates/engine/src/platform/boundary.rs).
- [Driver](crates/veyra/src/control.rs), [name coverage](crates/veyra/src/verify_map.rs), [verification skill](.agents/skills/verify-veyra/SKILL.md).

## Verification

The provider_options tests guard the repaired provider contract: offered_modes_and_run_mode, provider_mode_cycle_uses_only_offered_modes, list_providers_reports_offered_modes, ceiling_reads_parent_run_mode and child_requested_mode_is_checked_before_provider_run_mode cover the offered modes, requested child mode and parent's effective ceiling. claude_root_args_append_family_rules checks root-only family instructions; codex_initialize_opts_out_of_turn_diff checks both session and probe initialize.

Claude's claude_probe_args_turn_off_hooks_and_mcp and claude_probe_args_skip_strict_mcp_beside_managed_file check both policy branches. claude_probe_replay_uses_owned_managed_policy_path checks the explicit-environment boundary; claude_probe_env_turns_off_ide_and_connectors checks its child environment. claude_account_presence_and_source_types_control_sign_out and the claude_probe_signed_out_needs_token_source_none, claude_probe_api_key_reads_ready, claude_probe_third_party_reads_ready and claude_probe_silent_account_reads_ready replays cover the narrow signed-out rule.
Their Veyra-authored fixtures are probe-signed-out.jsonl, probe-api-key.jsonl, probe-third-party.jsonl and probe-no-account.jsonl. claude_403_shows_cli_text_and_keeps_session and claude_skipped_prompt_fails_turn replay result-403.jsonl and prompt-skipped.jsonl through following turns on the same process. login_shell_reads_mise_diff calls the platform function with a scripted shell; child_env_carries_mise_diff_with_path and failing_login_shell_keeps_inherited_path_and_mise_diff check the child snapshot and inherited fallback.

codex_root_config_sets_grace_and_tool_timeout and codex_rejected_optional_keys_retry_independently guard both optional keys and independent drop-key retries. codex_sign_in_failure_names_login_once replays sign-in-required.jsonl, whose unauthorized error shape is from Codex 0.162.1's TurnCompletedNotification schema; its message is Veyra-authored, not a recorded authentication failure. codex_unknown_error_kinds_keep_the_failure_message guards forward-compatible fallback. grok_turn_completed_ends_own_prompt_after_guard and grok_old_tail_never_ends_next_prompt run turn-completed-only.jsonl through the session timer; codec_guard_waits_once_and_ignores_late_reply and codec_untagged_tail_never_ends_next_prompt guard adverse ordering. grok_approval_deny retains the denial ending.

capture_keeps_none_and_masks_other_account_values and fixtures_hold_no_secret_shapes guard recording and fixture masking. keys_dispatches_down_then_up_per_keystroke guards the debug driver. chip_lists_offered_modes checks the composer. family_card_renders_user_status_words checks rendered card text; sidebar_tooltip_uses_shared_status_display is a GPUI wiring assertion that the tooltip uses the same display function, not a native tooltip capture. failed_banner_renders_own_cause_and_one_sign_in_instruction checks rendered causes and the absence/presence of sign-in text; failure_caption_defers_to_banner_and_preserves_history guards captions. actor_shutdown_during_missing_resume_spawns_no_replacement waits on marker and exit conditions with 30-second bounds, keeping its no-replacement and recovery assertions.

A live check on 2026-10-20 recorded the shapes on Claude 2.1.304, codex-cli 0.162.1 and Grok 1.0.62; the 403 frame and Codex's rejection wording rest only on Veyra-authored fixtures.
Its Claude lifecycle ran queued, started, result, completed, so completion without started remains fixture-only.
Grok's tagged completion, prompt_complete and reply arrived within the guard, so expiry and reordered untagged tails remain fixture-only.
Codex accepted both optional keys; no live rejection exercised their fallback.


multi_agent tests exercise actor rules, idempotent tasks, guidance, shared status words, settlement, held and successor wakes, grants, uncertain-admission recovery and retained writes and recovery notices.
mcp_socket uses the built relay and a fake CLI that records only its own argv, then checks foreign-peer refusal, the production relay with a wrong direct parent, a second relay from the same CLI reusing its token, private files and socket modes, RPC method fallback, instruction coverage and config cleanup.
Removing either the parent or one-use check makes its isolated relay regression fail.
mcp.rs launch_lock_tests blocks a launch closure and exercises revocation concurrently, with a bounded wait proving revocation does not wait for that closure.
mcp_socket also exercises production root session config lifetime, non-connected init cleanup, pre-init exit cleanup, child argv isolation and token redaction.

Real-relay socket and Codex and Grok family tests live in the application package, which receives CARGO_BIN_EXE_veyra and a fixture binary target from Cargo.
Both fixture binary entrypoints call the engine fixture::run dispatcher, which routes shell launches to run_tui and structured launches to cli::run.
Engine-only fake-session tests resolve their helper beside the current test profile instead of a fixed target/debug path.
No stale-binary timestamp guard or preparatory workspace build is required; the empty-target workspace test covers that ownership boundary.

Workspace tests cover private paths and modes, launch locking, platform_boundary, command-table behavior, recovery, storage, sessions and theme contrast. claude_stream_keeps_six_fixed_commits replays every stream-2000 frame through the codec and actor, checking three startup commits, three ending commits, five simulated two-second writes, retained version/text and the dispatch admission acknowledgement. version_metadata_waits_for_a_successful_write_or_flush checks metadata coalescing, the TextWrite flush handler, storage retry, commit-before-publish and no redundant flush.
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
Admission tests cover Stop, Interrupt, Configure, Quit, keep-awake, idle parking and refusal waiting for a CLI end.
A composer unit test checks the preview cut notice; a synthetic window checks that long approval text scrolls within a bounded area while decision buttons stay on-screen.
The approvals-questions recipe captures the real-window scroll area.
Grok window tests select Default through the actual dropdown menus and check that disabled approval clicks, keys and explicit actions send no response while Deny still sends its request id.
Codec and storage tests cover all tool arguments, symlinks with missing parents, unverified targets, truncated Unicode previews, legacy approval rows and undecodable control failures.
Shell tests inject an isolated provider environment and admit no process to their startup sweep.
Their current-thread runtimes do not drive readiness tasks before replacing the probes, so their startup probes start no provider check.
recheck_after_confirm_quit_launches_no_provider deliberately checks an owned stand-in as its positive control, without running a real provider.
Clippy with -D warnings and cargo fmt --check check source quality.
The platform boundary tests also scan source text for planning identifiers, Rust lines over 120 characters and host-dependent integration tests, each with planted-text self-tests.
The hermeticity scan exempts live tests and one named scripted-shell helper.
Its lexer preserves literal boundaries while excluding comments, and it rejects installed-provider command arguments and host-loading engine constructors.
Clippy disallows arbitrary JSON values outside documented wire-format allowances; tool arguments retain their existing wire JSON through a private narrowly allowed alias used only by that field.
`scripts/red-on-parent.sh BASE [EXPECT_PASS_FILE]` copies every changed test-only file, including fixtures and renamed paths, into an isolated archive of BASE and runs their changed tests by fully qualified name with exact matching in their own Cargo target.
A passing test fails the check unless the supplied evidence file names its reason; an absent or failing listed test makes that entry stale and fails the check.
Tests inside production source files retain manual red receipts.
The verify-veyra pages cover the real window, launch and relaunch row preservation, readiness without threads, streaming and background status, failure and Retry, default-mode persistence, header-branch clearing and cleanup.
The runner's check-only mode runs the gate, measured-pair and mocked backdrop ownership self-tests, and checks their dispatch with stubbed calls.
The ownership self-test starts no process and mocks every signal.
App-launch's release check proves that release has no driver, using string scans with a debug positive control, ctl refusal and socket checks.
Its macOS sandbox allows only the release Veyra executable and denies every child executable, including installed-provider probes; an unavailable sandbox, invalid profile or missing exec-denial receipt fails the check.
Approvals-questions, stop-interrupt and restart-recovery cover the dock, queue controls, banners, keep-awake, quit sheet and startup recovery.
The multi-agent page covers family controls, stored task guidance, delivery, single-child archive holds, archive choices, quiet titles and the visible/open timing probes.
The Codex sections of new-thread, chat-send-stream, approvals-questions and multi-agent cover CLI model/effort readiness, streamed text, approval and question docks, notices, foreign-thread filtering, the root relay and the Claude-root Codex child card.
The Grok sections of new-thread, chat-send-stream, approvals-questions and multi-agent cover Default-only fallback, initialize sign-in and catalog, per-model efforts, streaming, a background report ending, session authentication failure, ordinary and Deny-only approvals, both flat question spellings and a root relay result.
After Stop, the question leaves the dock and remains Expired in the request projection and SQLite; the turn shows Interrupted.
The fake lane supplies explicit overrides and probe fixtures for all three providers.
Permission-matrix records separately authorized live permission checks.
The ignored live tests require fresh human consent; ordinary workspace tests use only explicit stand-ins or missing provider paths in debug and release.
live_claude_approval_round_trip uses the public engine in Ask first to check an exact Bash request, Deny without a file, and AllowOnce on a repeat with the file created.
Actor tests check account-error retirement before queued work; a separate probe fixture checks Signed out while the no-account fixture remains Ready.
Live CLI behavior and performance need explicitly authorized checks; fake replay and structural checks cannot establish them.
Drawer tests use a scripted SHELL and the fake CLI's shell mode.
Ordinary tests do not open the user's shell or an installed provider TUI.
Real provider permission behavior, native keyboard/IME/pointer delivery and Open Terminal's OS side effect are not proved by fake replay or the debug driver.
Composer Stop sends Interrupt and sidebar StopThread sends StopThread; real-provider continuation requires explicitly authorized live verification.

Codex replay tests cover approvals, questions, elicitation, request cancellation order, resume errors and retries, all mode triples, early completion, foreign-thread filtering, notices and both command-output sources.
The mode-change replay sends Configure through the engine and completes one child conversation across four spawns.
The root proof uses the production veyra mcp relay, socket and peer checks, and the cross-provider proof settles a Codex child report into a Claude parent wake.
Peer tests cover omitted and included version fields, out-of-order replies, EOF tracking cleanup and the fatal line cap.
Relay tests hold the socket open while stdin or the parent ends and independently block socket and stdout writes before closing stdin.
The MCP half-close and queue-overflow tests hold an actor reply pending and require credential revocation and connection closure.
Codex live tests are ignored by default.
Codex and Grok live wrappers call the same provider-parameterized test bodies in tests/common/live.rs, and their replay tests use the shared Inputs, Input and isolated session harness in tests/common/replay.rs.

Grok replays cover prompt correlation, exactly-once completion, two turns per process, permissions, both question spellings, cancellation order, authentication retry, signed-out failure and advertised resume.
The title replay checks one persisted tool row with an updated title and preserved output.
Accept-edits regressions verify both edit docks and no automatic reply, including mismatched paths, protected paths and requests arriving after Stop; grant regressions reject Monitor and unknown keys.
Initialize regressions cover the seven reviewed null or mistyped variants plus malformed catalog containers and entries, preserving authentication and the probe sign-in reading.
A Provider-turn permission replay persists an untagged request and its human answer; an early-completion regression drops late content while allowing a fresh prompt.
Recorder regressions cover free-text email and bound hostname, account-key variants, other-folder session entries and MCP status detail.
Ordinary session-notification regressions require no logs while a malformed content update still logs its method.
Mode regressions verify false open metadata and a post-open notification for new, resumed and loaded Ask first and Accept edits sessions, with neither override in Full access.
A synthetic-window regression drives both actions and keys through the advertised models and efforts back to Default.
The ignored Grok approval round trip requests a printf redirect because bare touch is on Grok's safe list; Claude and Codex retain touch.
The first-frame timeout replay advances Tokio time and does not wait sixty seconds on the wall clock.
Actor-backed Grok replays verify persisted question answers and request expiry, and background work refusing Configure before a Provider continuation.
Foreign-request actor tests check permission and question docks, persisted answers and expiry, native cancellation before session/cancel, later Send after foreign-work promotion, both Stop targets, promotion after a stopped Veyra turn and overlapping Provider prompts.
Codex actor regressions verify a foreign native turn's permission docks under the bound Veyra turn, receives its native answer, or gets a cancel reply when that Veyra turn completes first.
Grok regressions verify null completion reasons end Provider work, invalid completion scopes leave it running, and a bad plan-entry status preserves the other entries.
The Grok child report regression drives decoded fixture frames through the actor and verifies the child's own reply reaches its parent despite later foreign content during that prompt.
The foreign-content unit regression checks payload-free logging and suppression during Veyra work, then normal content during Provider work.
Codex's untagged elicitation regression requires the next native prompt instead of an extra completion reply, while the foreign-request expiry regression still requires cancellation.
The foreign-tool replay preserves Bash and its input across promotion, fills partial start metadata from later frames, keeps the later output, and emits none of the other dropped content.
Malformed-notification replays keep the session alive through missing or numeric update kinds, a null tool title, a numeric prompt id and null metadata; codec tests check method-only diagnostics and prevent wrong-session or replay notifications from confirming admission.
grok_root_reaches_real_relay runs in the ordinary application suite with the production veyra mcp executable and fake CLI without opening a window.
A default credential-frame test asserts the Grok mcpServers entry, preserves unrelated config and requires only the token marker in its recording value.
The older recording-derived Grok fixtures name version 1.0.60 and use Veyra's masked live shapes for initialize, flat questions, task snapshots, Grok-started turn completion and resume.
Replay tests do not prove installed Grok behavior; the retained live recording supplies those observed wire shapes.
The live recording sent no permission requests, because Grok's settings-seeded auto mode approved them itself.
With auto and yolo mode turned off on every Ask first and Accept edits session, installed Grok 1.0.60 sends `session/request_permission`, and a Deny ends the turn cancelled with `PermissionRejected`.
The ignored live tests check that path on the installed CLI.
Grok live tests are ignored by default.

Shell tests cover login arguments and cwd, environment isolation, foreground/background close detection, the close and quit ladders, missing folders, per-thread closure, native quit latency, blocked actor independence, stalled synchronized output, large paste, worker panic and escaped PTY holders.
The three held-prompt cancellation tests retain prompt-before-interrupt coverage without native history cursors, and ordinary provider shutdown keeps its one-second writer drain.
stored_terminal_view_opens_in_chat checks that legacy flags create no hold, retain the stored TerminalSpan and leave the agent parked.
The store keeps schema 4 and writes chat, zero and null into the three unread legacy columns on thread upserts.
Window tests cover rail movement and bounds, observed composer/drawer/none focus, the focus table and draft caret, the tab cap, key precedence, real-key bracketed paste mode detection and control filtering, return from scrollback, hidden repaint suppression, legacy terminal-span dump text and the emulator's nonblocking key path.
The guarded shell-drawer page exercises native subscriptions, every drawer key from its required surface, close and archive consent, hidden output, restart, missing folders, orderly quit, crash hangup and one flood using owned fixtures.
Engine tests exercise the direct on_app_quit cleanup hook with live shells.
DebugDragShellDrawer dispatches synthetic mouse-down, held-left movement and mouse-up through the window rail handlers for the native recipe.
Synthetic clicks and control keys do not establish native pointer delivery, IME or the user's real shell plugins.
Real provider behavior still requires separately authorized live verification.

Migration and settings tests cover version-3 upgrade, unknown background refusal, persistence and SettingsChanged publication.
The display and theme tests cover forced Solid, every reduced-motion token and both appearances' contrast pairs, including sidebar tint over worst-case desktops.
The synthetic theme-apply test checks the retained Look projection across appearance changes; the native guarded captures establish the actual window material.
Drawer regressions cover a held drag cancelled by shrink and restoration of the chosen height after the composer shrinks.
The failed-shell spawn regression uses a real failing spawn, and the group guard regression requires signalling before reaping and no signal afterward.

The native_quit_releases_registry_before_shell_hangup regression lets a scripted shell exit at the native signal pass and bounds both reap observation and quit completion.
The forced_solid_keeps_background_choice_and_blocks_every_entry regression checks effective selection, click, action and key refusal under each forcing flag, with unforced controls.
