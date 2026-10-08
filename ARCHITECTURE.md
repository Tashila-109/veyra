# Veyra architecture

## Executive summary

Veyra currently opens one native macOS window with an empty project sidebar and a new-thread composer shell.
The gpui `Shell` entity owns the window state.
The `Theme` global owns its colours, typography, geometry, and motion tokens.
The engine library owns the data folder, instance lock, and macOS mechanisms through `platform`.
The engine never depends on gpui.

The target design lives in `.tstack/tasks/arch-plan/design.md` of the primary checkout.
That local file stays uncommitted.
This document describes M0 only.
There is no database, engine actor, provider process, or MCP relay.

### System architecture

```text
User -> macOS window -> Shell + Theme
                            |
main -> DataDir -> veyra.lock + logs/veyra.log + run/

debug ctl -> private Unix socket -> socket thread
          -> tokio mpsc receiver -> gpui task -> window actions and keys
```

### Dependency hierarchy

```text
veyra -> veyra-engine -> libc + Rust standard library
veyra -> gpui-kit -> gpui-pre
veyra -> tokio sync + serde JSON + tracing
```

Only the `veyra` binary depends on gpui.
`crates/engine/Cargo.toml` enforces that boundary.

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

`DataDir::lock` opens `veyra.lock` through `platform::private_file` at mode 0600 and retries `platform::lock_exclusive` for one second.
Only the lock holder writes its pid or removes a stale debug socket.
A second launch prints `Veyra is already running` and exits 1.
The open file stays alive across the UI loop.
The OS releases its lock when the process exits or crashes.

After locking, `main` opens the append-only `logs/veyra.log` through `platform::private_file` at mode 0600 and installs `tracing-subscriber` at INFO.
M0 does not read `RUST_LOG`, so an environment filter cannot suppress readiness.
Debug builds bind the driver through `platform::bind_socket` before opening the window.
`Window::on_next_frame` prints the ready line to stderr and the log after the first rendered frame.
`build.rs` embeds Git HEAD, or `unknown` without Git, and tracks HEAD and branch refs for rebuilds.

## Window state and actions

`Shell` owns its focus handle, sidebar width, resize gesture, and optional notice.
The sidebar begins at 256 px and clamps to 224 through 400 px.
Both drag and keyboard resizing dispatch `SetSidebarWidth`.
The header is 44 px.
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
A window-appearance observer reapplies the full theme when macOS changes appearance.

## Debug protocol and trust boundary

The whole `control` module and its argument branch exist only under `cfg(debug_assertions)`.
The socket sits at `DATA/control.sock` with mode 0600 inside the private data folder.
This restricts it to the current user.
There is no additional authentication against processes running as that user.

The client connects through `platform::connect_socket` and sends one JSON line containing a tagged `Command`.
The server limits each line to 65536 bytes and gives socket I/O and UI replies a two-second timeout.
A bounded tokio channel holds at most 16 requests.
A full or closed queue rejects the request before enqueueing it.
A reply timeout does not cancel an enqueued request.
Its action can still execute later, so inspect the window state before retrying.
The socket thread performs no gpui work.
One `cx.spawn_in` task receives each request and uses the window's action builder and key dispatch.

`action` accepts an action name and optional JSON data.
`keys` parses all keystrokes before dispatch.
`text` returns an error naming the M0 composer shell.
`dump` reads the entity and theme.
It reports pid, build revision, data folder, appearance, view, sidebar width and rows, composer text, and the current notice.
`quit` dispatches the same `Quit` action as the menu.
The client prints each reply and exits 1 for a driver error.

The ready line and driver are development checks, not external service APIs.
A stale socket can remain after quit because macOS termination does not promise Rust destructors.
The next lock holder removes it.
Verification cleanup stops its own process before deleting its scratch folder.

## Source map

- [Data folder and lock](crates/engine/src/data_dir.rs) owns path validation, pid stamping, and launch exclusion.
- [Platform selection](crates/engine/src/platform/mod.rs) selects macOS, rejects other targets, and holds `Signal`.
- [macOS mechanisms](crates/engine/src/platform/macos.rs) owns permissions, file locking, and socket operations.
- [Platform boundary tests](crates/engine/src/platform/boundary.rs) checks Rust files and its own matcher.
- [Entry point](crates/veyra/src/main.rs) owns argument routing and logging.
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
The `app-launch` feature recipe checks the ready line, driver identity, window, log, instance refusal, actions, and cleanup.
The release check confirms `ctl dump` exits 2 and the binary contains no driver socket name.
Native keyboard delivery, IME, pointer dragging, and a manual macOS appearance switch need a human check.
The debug driver does not prove those OS paths.
