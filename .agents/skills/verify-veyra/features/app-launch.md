# App launch

Veyra opens one window with an empty project sidebar and a new-thread composer shell.
The same data folder accepts only one instance.
The debug CLI identifies the window without external Accessibility input.

## Sub-features

- `launch-window` shows the empty sidebar, Add project, header, question, and composer in Geist.
- `launch-ready` prints `veyra ready pid=PID data_dir=DIR` after the first frame.
- `launch-identity` reports the pid, revision, data folder, appearance, view, sidebar, and empty composer.
- `launch-lock` refuses a second process on the same data folder.
- `launch-log` writes the ready line and action notices to `DATA/logs/veyra.log`.
- `launch-controls` reaches shell controls and sidebar resizing through named actions and key bindings.
- `launch-quit` closes the window and leaves proof after cleanup.

## How to get to it (user POV)

- Run `cargo run -p veyra` with no argument.
- Choose Add project in the sidebar.
- Choose New thread in the header, or press ⌘N.
- Choose Claude Code, Model, High, Ask first, or Send in the composer shell.
- Drag the sidebar divider, or press ⌘⌥Left and ⌘⌥Right.
- Choose Quit Veyra in the native menu, or press ⌘Q.
The driver reaches those gpui paths.
Native mouse and OS key delivery remain manual checks.

## Driving it with veyra ctl

Preconditions:

- Launch and Doctor passed with the matching `DATA`, `PID`, `ID`, and `PROOF`.
- Every command runs after `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`.

1. Capture the baseline.
Run `VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl dump >"$PROOF/dump-before.json"`.
Require `view == "new_thread"`, sidebar width 256, no rows, and empty composer text.
Require the ready line in `stderr.log`.

2. Check the instance lock.
Run `VEYRA_DATA_DIR="$DATA" target/debug/veyra >"$PROOF/second-launch.txt" 2>&1; printf 'exit=%s\n' "$?" >>"$PROOF/second-launch.txt"`.
Require `Veyra is already running` and `exit=1`.
The first pid must still answer Doctor.

3. Reach Add project.
Run `VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl action veyra::AddProject`.
Then dump to `add-project.json`.
Require `Adding projects is not available in M0.` in the notice and still no rows.
Also run `ctl keys secondary-shift-o` and require the same notice.

4. Reach the composer controls.
Run `ctl action veyra::PickProvider`, `ctl action veyra::PickModel`, `ctl action veyra::PickEffort`, `ctl action veyra::PickPermission`, and `ctl action veyra::Send` with the same environment.
Dump after each command.
Each reply is `{"ok":true}`.
The notice names the unavailable shell operation.
Run `ctl keys "secondary-alt-p secondary-alt-m secondary-alt-e secondary-alt-a secondary-enter"` and require the Send notice.
No provider process or project row appears.

5. Check resizing and data-bearing actions.
Run `ctl action veyra::SetSidebarWidth '{"width":320}'`, then `ctl keys secondary-alt-left`.
Require width 304 in the next dump.
Run `ctl action veyra::SetSidebarWidth '{"width":100}'` and require 224.
Run it with `{"width":900}` and require 400.
Run `ctl keys secondary-alt-0` and require 256.

6. Check text refusal and New thread.
Run `ctl text "hello"` and save stdout, stderr, and exit code.
Require an error naming `the M0 composer is a shell with no text input` and exit 1.
Run `ctl action veyra::NewThread` and `ctl keys secondary-n`.
Require a null notice, empty composer, and `new_thread` view.

7. Capture the window and log through the Evidence section.
Require the log's ready line to name this pid and data folder.
Require `dump-after.json`, `window.png`, `veyra.log`, and `second-launch.txt`.
Inspect the PNG for the empty sidebar and composer shell.

8. Run Cleanup.
`ctl quit` must use the same Quit action as the menu.
After cleanup, `ls "$PROOF"` still lists the proof and `test ! -e "$DATA"` passes.

## Gotchas

- M0 is a shell.
Add project, pickers, and Send show notices.
They do not create projects, threads, or provider sessions.
- The composer has no text input in M0.
A text-command error is the expected result, not a failed input-handler implementation.
- Change macOS appearance by hand to check dark and light.
Veyra must follow in Geist.
The driver cannot change system appearance.
Do not mark this manual path verified from the contrast test.
- Accessibility is not granted to agent tools on this Mac.
A driver key proves gpui dispatch, not the OS keypress.
- A socket path permits at most 103 usable bytes.
Keep run IDs short.
- `bin/winid.swift` and `screencapture` run only on macOS.
A port replaces them with its own capture tools.
- Window managers can change the initial window bounds.
Capture `winid.swift PID --bounds` instead of assuming the requested dimensions.
- Unknown arguments, including `mcp`, print `usage: veyra [mcp]` and exit 2.
Release builds have no `ctl` driver.
