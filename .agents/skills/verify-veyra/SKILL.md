---
name: verify-veyra
description: Verify Veyra's native macOS window and CLI after UI, launch, logging, data-folder, or debug-driver changes. Launch an isolated instance, drive gpui actions through veyra ctl, capture the window, and preserve evidence after cleanup.
---

# Verify Veyra

The primary user interface is one native macOS gpui window.
Use the debug driver because external Accessibility input is not granted on this Mac.
Read [the feature map](features/README.md) before choosing a recipe.
Run every command from this checkout in a bash shell.

## Launch

Screen Recording must be granted to the host that runs `screencapture`.
Rust comes from Homebrew rustup.
Each run owns a short scratch path and its process group.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
RUN=$(uuidgen | cut -c1-8 | tr A-Z a-z)
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
test ! -e "$DATA" && test ! -e "$PROOF" || exit 1
cargo build --workspace || exit 1
mkdir -p "$PROOF"
set -m
VEYRA_DATA_DIR="$DATA" target/debug/veyra >"$PROOF/stdout.log" 2>"$PROOF/stderr.log" &
PID=$!
PGID=$(ps -o pgid= -p "$PID" | tr -d ' ')
printf '%s\n' "$PID" >"$PROOF/pid"
printf '%s\n' "$PGID" >"$PROOF/pgid"
test "$PID" = "$PGID" || exit 1
for attempt in $(seq 1 100); do
	grep -q "^veyra ready pid=$PID data_dir=$DATA$" "$PROOF/stderr.log" && break
	kill -0 "$PID" 2>/dev/null || break
	sleep .1
done
grep -q "^veyra ready pid=$PID data_dir=$DATA$" "$PROOF/stderr.log"
```

The last grep must exit 0.
If launch fails, run Cleanup before reporting.
Do not use the default data folder.
M0 runs no provider and needs no login or database.

## Doctor

Run Doctor first whenever the instance looks wrong.
Do not drive an instance whose pid, revision, data folder, or window does not match this run.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl dump >"$PROOF/doctor.json"
python3 - "$PROOF/doctor.json" "$DATA" "$PID" <<'PY'
import json, pathlib, subprocess, sys
dump = json.load(open(sys.argv[1]))
assert dump["data_dir"] == sys.argv[2]
assert dump["pid"] == int(sys.argv[3])
assert pathlib.Path(sys.argv[2], "veyra.lock").read_text().strip() == sys.argv[3]
assert dump["build_rev"] == subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
assert dump["view"] == "new_thread"
print("doctor identity passed")
PY
ID=$(swift .agents/skills/verify-veyra/bin/winid.swift "$PID")
test -n "$ID"
printf '%s\n' "$ID" >"$PROOF/window-id"
swift .agents/skills/verify-veyra/bin/winid.swift "$PID" --bounds >"$PROOF/bounds.txt"
```

The client has a two-second read timeout.
A wrong identity or unavailable window fails Doctor.
A build outside Git reports `unknown` instead of HEAD.

## Drive

Use the commands in [app-launch](features/app-launch.md).
`action NAME [JSON]` builds a registered gpui action.
`keys KEYSTROKES` dispatches each space-separated key through the window.
Both paths are the same dispatch paths the window uses.
M0's `text STRING` returns an error because its composer has no input handler.
`dump` reads the rendered state.
`quit` dispatches the menu's Quit action.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl action veyra::AddProject
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl action veyra::SetSidebarWidth '{"width":320}'
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl keys "secondary-alt-left secondary-n"
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl dump
```

No coordinates or direct state setters are needed.
Shell controls report that M0 does not yet implement the operation.
A reply timeout does not cancel an enqueued action.
Run Doctor and inspect the current dump before retrying.
A full-queue error means that request was not enqueued.

## Evidence

Capture the action and its resulting state.
Keep dump JSON before and after, the ready line, second-launch stderr and exit code, the app log, and the window.
Use the pid filter so another Veyra window cannot supply the screenshot.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl dump >"$PROOF/dump-after.json"
screencapture -x -o -l "$ID" "$PROOF/window.png"
cp "$DATA/logs/veyra.log" "$PROOF/veyra.log"
{ git rev-parse HEAD; git status --short; } >"$PROOF/identity.txt"
```

Inspect the PNG itself.
Require the empty sidebar, 44 px header, question, and composer shell.
A dump alone does not prove visible layout or fonts.
M0 has no database or network side effects.
Do not count native typing, pointer dragging, or a manual theme change as passed through a driver action.

## Cleanup

Run cleanup after successful and failed attempts.
Recover `PID` and `PGID` from this run's proof folder if the shell changed.
Signal only the processes this run created.
Copy the log before removing scratch state.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
test ! -f "$DATA/logs/veyra.log" || cp "$DATA/logs/veyra.log" "$PROOF/veyra.log"
if kill -0 "$PID" 2>/dev/null; then
	VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl quit >"$PROOF/quit.json" 2>"$PROOF/quit.stderr"
fi
for attempt in $(seq 1 50); do
	kill -0 "$PID" 2>/dev/null || break
	sleep .1
done
if kill -0 "$PID" 2>/dev/null; then
	kill -KILL -- "-$PGID"
fi
wait "$PID" 2>/dev/null || true
if kill -0 "$PID" 2>/dev/null; then
	printf '%s\n' "cleanup failed; keep $DATA for diagnosis" >&2
	exit 1
fi
test "$DATA" = "/tmp/veyra-verify-$RUN" || exit 1
rm -rf "$DATA"
test ! -e "$DATA"
ls -la "$PROOF" >"$PROOF/cleanup-listing.txt"
test -s "$PROOF/window.png" && test -s "$PROOF/doctor.json"
```

Cleanup leaves `target/verify-veyra/$RUN/` intact.
Never remove that folder as part of scratch cleanup.
Do not kill by process name.

## Helpers

`bin/winid.swift` is executable.
Run `swift .agents/skills/verify-veyra/bin/winid.swift PID` for a window number.
Add `--bounds` for `x,y,w,h`, suitable for `screencapture -x -V 15 -R BOUNDS`.
It filters on owner pid and the normal on-screen window layer.

## Maintain the map

Use `/maintain-verification-skill` when commands, actions, or user entry points change.
Update the matching feature recipe in the same change.
