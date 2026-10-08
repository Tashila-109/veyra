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

`DataDir::lock` opens `veyra.lock` through `platform::private_file` at mode 0600 and retries `platform::lock_exclusive` for four seconds.
That wait allows a relaunch to outlast the planned three-second Quit grace period.
M0's Quit still exits immediately.
Only the lock holder writes its pid or removes a stale debug socket.
A second launch prints `Veyra is already running` and exits 1.
The open file stays alive across the UI loop.
The OS releases its lock when the process exits or crashes.

After locking, `main` opens the append-only `logs/veyra.log` through `platform::private_file` at mode 0600 and installs `tracing-subscriber` at INFO.
M0 does not read `RUST_LOG`, so an environment filter cannot suppress readiness.
Debug builds bind the driver through `platform::bind_socket` before opening the window.
`Window::on_next_frame` prints the ready line to stderr and the log after the first rendered frame.
`build.rs` embeds Git HEAD, or `unknown` without Git.
It watches existing Git HEAD, HEAD reflog, packed refs, and the current loose branch ref.
HEAD changes on a branch switch.
The HEAD reflog changes on a commit even when the loose branch ref did not exist at build time.

## Window state and actions

`Shell` owns its focus handle, sidebar width, resize gesture, and optional notice.
The sidebar begins at the theme's 240 px default and clamps to 200 through 448 px.
Both drag and keyboard resizing dispatch `SetSidebarWidth`.
The keyboard step is the theme's 16 px.
The width-reset binding dispatches `ResetSidebar`, whose handler reads the current theme default before dispatching `SetSidebarWidth`.
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
`crates/veyra/assets/fonts/` holds the regular, italic, medium, and semibold Geist faces and regular Geist Mono from [Vercel's official Geist v1.7.2 release](https://github.com/vercel/geist-font/releases/tag/v1.7.2).
The release's own `OFL.txt` licence sits beside those files.
A window-appearance observer reapplies the full theme when macOS changes appearance.

### Theme numbers

`ui/theme.rs` owns the requested 1280 by 800 px window and 760 by 560 px minimum.
A window manager can override the initial outer bounds.
The composer caps at 784 px, with a fixed 48 px placeholder area, 40 px toolbar, and 16 px corner radius.
The 240 px maximum text-height token is stored but unused in M0.
The transcript tokens are 480, 752, and 752 px, with 40 px gutters.
M0 does not render a transcript or drawer.
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
`dump` reports the process pid, embedded build revision, data folder, theme appearance, entity sidebar width, and current notice.
In M0, `view` is fixed to `new_thread`, `sidebar.rows` to `[]`, and `composer_text` to an empty string.
Those fixed fields do not prove rendered state.
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
The [release check in app-launch](.agents/skills/verify-veyra/features/app-launch.md#release-check) verifies that `ctl dump` exits 2, the running release instance creates no `control.sock`, and its binary contains no driver socket name.
It uses the debug binary as a positive control for the string scan.
Native keyboard delivery, IME, pointer dragging, and a manual macOS appearance switch need a human check.
The debug driver does not prove those OS paths.
