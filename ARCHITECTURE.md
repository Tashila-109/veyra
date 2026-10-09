# Veyra architecture

## Executive summary

Veyra currently opens one native macOS window with an empty project sidebar and a new-thread composer shell.
The gpui `Shell` entity owns the window state.
The `Theme` global owns its colours, typography, geometry, and motion tokens.
The engine owns the data folder, instance lock, SQLite projection, and orchestration actor.
Startup opens and migrates the database before constructing the actor.
After startup, the actor owns the connection and conversation state and is the sole database writer.
The engine never depends on gpui.

The window is still the M0 shell.
M1 starts the engine before the window opens.
There is no real provider process, PTY, or MCP relay.

### System architecture

```text
User -> macOS window -> Shell + Theme
                            |
main -> tokio runtime + Engine::start -> instance lock + migration + reconcile
                                    -> engine actor -> veyra.db
main -> logs/veyra.log + macOS window
engine actor -> normalized provider channels -> test-only scripted sessions

debug ctl -> private Unix socket -> socket thread
          -> tokio mpsc receiver -> gpui task -> window actions and keys
```

### Dependency hierarchy

```text
veyra -> veyra-engine -> rusqlite (bundled) + tokio (sync, rt-multi-thread, time, macros)
                     -> serde (derive) + serde_json + uuid (v5, v7, serde)
                     -> thiserror + tracing
                     -> platform -> libc + Rust standard library
veyra -> gpui-kit + gpui-pre
veyra -> tokio (sync, rt-multi-thread, time) + serde (derive) + serde_json
      -> tracing + tracing-subscriber + anyhow
```

Only the `veyra` binary depends on gpui.
`crates/engine/Cargo.toml` declares that boundary by omitting gpui dependencies.
The compiler refuses an undeclared crate.

## Platform ownership

`crates/engine/src/platform/` owns the macOS data-folder default, socket path limit, exclusive lock call, private file and folder modes, and socket bind and connect.
These are plain functions in `macos.rs`.
Callers choose file-open policy through standard `OpenOptions`.
`lock_exclusive` reports standard I/O errors, so the data-folder code owns the retry and instance-refusal policy.
`bind_socket` removes a stale path before binding and applies mode 0600.
The driver infers socket values and handles protocol I/O through `Read + Write`.
The platform module owns no UI or command protocol.

`mod.rs` holds the shared `Signal` enum and selects `macos.rs` only for macOS.
Other targets stop at a `compile_error!` naming `crates/engine/src/platform/`.
No other file calls macOS APIs directly.
gpui owns the native window.
Modified key bindings use gpui's `secondary-` modifier, which means Command on macOS and Control on other platforms.
Escape remains an unmodified key.

`boundary.rs` enforces the call boundary with `platform_boundary`.
It scans every Rust file under `crates/` outside the platform folder.
It blanks line and block comments without removing line breaks, matches `libc` as a whole word, and checks the approved platform-specific needles.
A violation names its file, line, and matching token.
`platform_boundary_self_test` covers grouped and aliased imports, conditional compilation, commands, comments, and a portable file-descriptor import.
This textual check misses indirect commands, full executable paths, generic permission calls, and text after `//` inside a string.
A `/*` inside a string literal makes the matcher treat later text as a block comment.
Without a later `*/`, it blanks the rest of the file.
Review still checks those cases.
The Swift window finder and `screencapture` in verify-veyra run only on macOS.
A port replaces those capture tools.

## Startup and data ownership

`main::run` checks arguments before creating folders or starting gpui.
No argument opens the window.
Debug builds route `ctl` to the client.
All other arguments, including `mcp`, print `usage: veyra [mcp]` and exit 2.

`DataDir::resolve` uses `VEYRA_DATA_DIR`, or `~/Library/Application Support/Veyra` when unset.
Relative overrides become absolute paths.
It validates the longest socket path before creating the data folder, `logs/`, and `run/`.
`platform::private_dir` sets each folder to mode 0700.
Debug builds validate `control.sock`.
Release builds validate `veyra.sock` but do not create a listener.
More than 103 usable bytes is an error that names the path.

`Engine::start` calls `DataDir::lock`, opens and migrates `veyra.db`, reconciles stored work, and starts the actor.
The engine keeps the instance lock alive on the actor's thread.
`main` creates a multi-thread tokio runtime for session tasks and passes its handle to the engine.
The engine does not enter that runtime on the UI thread.
The runtime stays alive across the UI loop.
M1 performs no process reaping.

`DataDir::lock` opens `veyra.lock` through `platform::private_file` at mode 0600 and retries `platform::lock_exclusive` for four seconds.
That wait allows a relaunch to outlast the planned three-second Quit grace period.
M0's Quit still exits immediately.
Only the lock holder writes its pid or removes a stale debug socket.
A second launch prints `Veyra is already running` and exits 1.
The actor owns the open lock file across the UI loop.
The OS releases its lock when the process exits or crashes.

After the engine starts, `main` opens the append-only `logs/veyra.log` through `platform::private_file` at mode 0600 and installs `tracing-subscriber` at INFO.
M0 does not read `RUST_LOG`, so an environment filter cannot suppress readiness.
Debug builds bind the driver through `platform::bind_socket` before opening the window.
`Window::on_next_frame` prints the ready line to stderr and the log after the first rendered frame.
`build.rs` embeds Git HEAD, or `unknown` without Git.
It watches existing Git HEAD, HEAD reflog, packed refs, and the current loose branch ref.
HEAD changes on a branch switch.
The HEAD reflog changes on a commit even when the loose branch ref did not exist at build time.

## Window state and actions

`Shell` owns its focus handle, sidebar width, resize gesture, and optional notice.
The sidebar reads its default, bounds, and keyboard step from `Theme::new`.
Both drag and keyboard resizing dispatch `SetSidebarWidth`.
The width-reset binding dispatches `ResetSidebar`, whose handler reads the current theme default before dispatching `SetSidebarWidth`.
There are no sidebar rows.
The composer has no text input.
Shell-only controls show a notice instead of creating project or provider state.

Every control dispatches a gpui action.
`ui/actions.rs` owns their key bindings and the native Quit menu.
Both the menu and ⌘Q dispatch `Quit`, which calls `cx.quit()`.

`Theme::apply` changes the kit to Dark or Light before projecting Veyra's tokens through the kit's `Theme::update`.
The kit's hover accent stays neutral.
Violet paints interaction roles.
Bundled fonts register through `add_fonts`.
`crates/veyra/assets/fonts/` holds the regular, italic, medium, and semibold Geist faces and regular Geist Mono from [Vercel's official Geist v1.7.2 release](https://github.com/vercel/geist-font/releases/tag/v1.7.2).
The release's own `OFL.txt` licence sits beside those files.
A window-appearance observer reapplies the full theme when macOS changes appearance.

### Theme tokens

`Theme::new` in `ui/theme.rs` owns the window, sidebar, header, composer, transcript, type, spacing, radius, and motion numbers.
A window manager can override the initial outer bounds.
The shell does not render a transcript or drawer.
The theme has no drawer tokens.
The sidebar, header, native traffic-light inset, structural borders, button geometry, and button labels read theme values rather than owning pixel literals.
Button labels are styled children with explicit accessibility names, so the kit's default label size does not override Veyra's type role.

`Theme::new` is the source of truth for type roles, spacing, radii, and motion tokens.
Type roles share a 16 px rem base with the kit.
M0 stores motion tokens but does not animate its shell.
The dark surface order is terminal, background, shell, card, overlay, and raised.
The violet accent and selection colours are the human's named exception in `AGENTS.md`.

## Debug protocol and trust boundary

The whole `control` module and its argument branch exist only under `cfg(debug_assertions)`.
The socket sits at `DATA/control.sock` with mode 0600 inside the private data folder.
This restricts it to the current user.
There is no additional authentication against processes running as that user.

The client connects through `platform::connect_socket` and sends one JSON line containing a tagged `Command`.
The server limits each line to 65536 bytes.
One socket thread serves connections serially, including command reading and the UI-reply wait.
Socket timeouts bound individual blocking I/O waits to two seconds, not the whole line-read duration.
A same-user client that keeps sending bytes within that timeout blocks later clients until its line completes or exceeds the byte limit.
UI replies have a separate two-second timeout.
A bounded tokio channel holds at most 16 requests.
A full or closed queue rejects the request before enqueueing it.
A reply timeout does not cancel an enqueued request.
Its action can still execute later, so inspect the window state before retrying.
The socket thread performs no gpui work.
One `cx.spawn_in` task receives each request and uses the window's action builder and key dispatch.

`action` accepts an action name and optional JSON data.
`keys` parses all keystrokes before dispatch.
`text` returns an error naming the M0 composer shell.
`dump` reports the process pid, embedded build revision, data folder, theme appearance, entity sidebar width, current notice, and migrated `schema_version`.
The driver adds `schema_version` from the engine started in `main`, without opening another database connection.
In M0, `view` is fixed to `new_thread`, `sidebar.rows` to `[]`, and `composer_text` to an empty string.
Those fixed fields do not prove rendered state.
`quit` dispatches the same `Quit` action as the menu.
The client prints each reply and exits 1 for a driver error.

The ready line and driver are development checks, not external service APIs.
A stale socket can remain after quit because macOS termination does not promise Rust destructors.
The next lock holder removes it.
Verification cleanup stops its own process before deleting its scratch folder.

## Engine actor and provider boundary

`actor::Actor` runs on its own `std::thread` and owns the SQLite connection, projection rows, live-thread map, text buffers, and event counters.
It blocks on a tokio message receiver with a capacity of 256.
A tokio timer sends `Tick` every five seconds.
Streaming text schedules separate flush and write messages.
The actor never waits for provider I/O.
The UI-facing Engine handle sends typed commands and returns oneshot acknowledgements.
Once enqueued, a subscription begins with the stored `SettingsChanged` row on an unbounded receiver.
An enqueued snapshot request returns stored items, unpersisted text, and the thread's event counter.
If that thread has text awaiting UI delivery, its reply waits for the scheduled text flush, without blocking the actor.
The snapshot counter then includes that text prefix.
The subscription does not filter overlapping events for a snapshot consumer.
A consumer must enqueue its subscription before requesting the snapshot and retain arriving events while the snapshot waits.
It installs the stored items plus buffered text, then applies only events for that thread with counters above the snapshot counter.
It handles settings events and storage-blocked notifications separately because those carry no thread counter.
`snapshot_joins_buffer_without_duplicate_text` checks this join rule.
A full or closed actor queue returns `Busy` for a command, closes a subscription without an initial event, and cancels a snapshot reply.
The current shell does not subscribe to engine events or send conversation commands.

`commands::decide` reads the projection and live state and returns a `ChangeSet` of changed rows, events, runtime replacements, and effects.
The actor completes buffered text for ended turns and finalizes summaries only for tasks newly settled in that decision.
A settled summary remains unchanged when its delivery changes.
Start effects name their turns until the actor builds their controls from those completed rows.
The actor then opens one transaction, calls `store::apply`, and commits.
Only then does it install runtime changes, publish numbered change events, and run effects.
Thread-scoped changes and text appends each advance that thread's counter.
Text deltas coalesce per item, normally flushing at most once per 16 ms.
Requests and completion events flush pending text immediately.
Open item text writes every two seconds and when its turn ends.
A command write failure returns `CommandError` without publishing its decision.
A provider or terminal write failure retains the failed changes, interrupts live sessions, refuses commands, and publishes `StorageBlocked`.
Each Tick retries that write before draining deferred process events.

`LiveThread` keeps its epoch outside the `Live` variants.
A spawn increments the epoch, and stale session events do not apply.
Turn events must also match the running turn.
Uncorrelated start or content events create a Provider turn only when no turn runs.
Further uncorrelated content joins that Provider turn.
Idle structured sessions park after ten minutes without a running turn, pending request, or background work.
Hold and wait reasons are typed.
Stop and Archive decide children first in one transaction.
Continue can discard queued user turns while retaining task and wake turns.

`provider` exposes normalized session and turn events, control channels, and terminal-handle types.
It does not expose CLI wire data to the actor.
`ProviderKind::spawn` returns `NotBuilt` for all three providers.
`Spawner::Scripted` exists only in tests and starts no OS process.
Agent tool calls return a not-built result in M1.
There is no real CLI, PTY, interrupt ladder, or keep-awake guard yet.

## SQLite projection and recovery

`store::MIGRATIONS` owns schema 1.
The database uses WAL, `synchronous=NORMAL`, foreign keys, and a two-second busy timeout.
`Engine::start` creates the database file through `platform::private_file` at mode 0600.
Migrations run by `PRAGMA user_version` in transactions and refuse a newer schema.
Times in the projection are unix milliseconds.
The eight tables are `projects`, `threads`, `turns`, `items`, `requests`, `grants`, `tasks`, and `settings`.
Rows are the source of truth.
Change events, live states, derived statuses, and wait reasons are not stored as an event log.
User text lives once in an item, referenced by its turn.
`store::apply` also owns live pid writes and leaves `live_pgid` null until a process group exists.
An archived thread's later archived writes preserve its first `archived_at`.
`ChangeSet` reads items, requests, tasks and turns through one pending-row overlay.
Wake queueing and hold updates share command helpers with recovery.
The shared turn-ending path replaces unaccepted wakes for provider events and user commands.
Archived parents retain queued reports and never restart through Continue.

A root thread stores its own id as `root_thread_id`.
Two lineage CHECKs and the composite parent/root foreign key refuse grandchildren, roots naming another root, and child-under-child updates.
The hold CHECK permits only the four HoldReason values or null.
The singleton settings row starts in `full_access` and constrains the three permission modes.
Thread modes remain in typed `config_json` and do not change when the default changes.

`reconcile::decide` runs once in one transaction before commands are accepted.
Running turns become interrupted, pending requests expire, and affected queues hold as Restarted without replacing an earlier hold reason.
Running child tasks stay open.
An unaccepted wake that already started gets a replacement queued wake.
A wake that never started keeps its id.
Terminal view returns to Chat with `import_pending`.
No provider work restarts automatically.
M1 clears no orphan process by signal and writes no CLI configuration.

## Source map

- [Data folder and lock](crates/engine/src/data_dir.rs) owns path validation, pid stamping, and launch exclusion.
- [Platform selection](crates/engine/src/platform/mod.rs) selects macOS, rejects other targets, and holds `Signal`.
- [macOS mechanisms](crates/engine/src/platform/macos.rs) owns permissions, file locking, and socket operations.
- [Platform boundary tests](crates/engine/src/platform/boundary.rs) checks Rust files and its own matcher.
- [Entry point](crates/veyra/src/main.rs) owns argument routing, runtime lifetime, engine startup, and logging.
- [Engine handle](crates/engine/src/lib.rs) owns startup, channels, subscription and snapshot entry points.
- [Actor](crates/engine/src/actor.rs) owns the connection, commit/publish order, stream buffers, epochs and storage retry.
- [Commands](crates/engine/src/commands.rs) owns command decisions, start-rule checks, cascades and task settlement.
- [Domain](crates/engine/src/domain.rs) owns typed rows, lineage, permission modes and derived statuses.
- [Live state](crates/engine/src/live.rs) owns runtime variants, start eligibility and parking.
- [Store](crates/engine/src/store.rs) owns pragmas, migration 1 and typed projection persistence.
- [Recovery](crates/engine/src/reconcile.rs) owns idempotent startup decisions.
- [Events](crates/engine/src/events.rs) owns row changes, effects, change events and snapshot data.
- [Provider boundary](crates/engine/src/provider/mod.rs) owns normalized controls and events.
- [Engine tests](crates/engine/src/tests/mod.rs) owns fixtures, table cells, recovery, storage, session events and the ignored perf probe.
- [Verification coverage](crates/veyra/src/verify_map.rs) checks source names against the feature map.
- [Build identity](crates/veyra/build.rs) embeds Git HEAD.
- [Window](crates/veyra/src/ui/mod.rs) owns shell state and the ready callback.
- [Actions](crates/veyra/src/ui/actions.rs) owns keys and the native menu.
- [Theme](crates/veyra/src/ui/theme.rs) owns all token groups and font registration.
- [Driver](crates/veyra/src/control.rs) owns the debug socket protocol.
- [Verification skill](.agents/skills/verify-veyra/SKILL.md) drives the real window and preserves proof.

## Verification

`cargo test --workspace` checks `socket_path_limit`, `lock_refuses_second`, private folder, lock-file, and socket modes, stale-socket removal, protocol parsing, `theme_contrast`, `platform_boundary`, and `platform_boundary_self_test`.
The checked-in `theme_contrast` pair list is the coverage source of truth.
It checks both appearances with a 4.5 text floor and a 3.0 accent, status, and dim-terminal-colour floor.
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` check the code.
The `app-launch` feature recipe checks the ready line, driver identity, window, log, instance refusal, actions, migrated database, quit and crash relaunches, and cleanup.
The engine's 66 `table_` tests cover the command-by-state cells.
Named session, recovery, storage, lineage, permission, hold and setting tests exercise committed rows and controls.
`verify_map_names` requires each action, driver verb and top-level dump key in the map.
The ignored release `perf_decide_commit` runs 10,000 Send commands on a file-backed database with production pragmas and writes raw samples under `target/`.
The independent architecture review is required before this document counts as approved.
The [release check in app-launch](.agents/skills/verify-veyra/features/app-launch.md#release-check) verifies that `ctl dump` exits 2, the running release instance creates no `control.sock`, and its binary contains no driver socket name.
It uses the debug binary as a positive control for the string scan.
Native keyboard delivery, IME, pointer dragging, and a manual macOS appearance switch need a human check.
The debug driver does not prove those OS paths.
