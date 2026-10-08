---
name: verify-veyra
description: Verify Veyra's native macOS window and CLI after UI, launch, logging, data-folder, or debug-driver changes. Launch an isolated instance, drive gpui actions through veyra ctl, capture the window, and preserve evidence after cleanup.
---

# Verify Veyra

The primary user interface is one native macOS gpui window.
Use the debug driver because external Accessibility input is not granted on this Mac.
Read [the feature map](features/README.md) before choosing a recipe.
Run every block from this checkout in bash.
Each section works in a fresh bash process.
Launch prints `RUN` and records it in `target/verify-veyra/latest`.
Later sections recover that identifier and read the process and window records from its proof folder.
To resume a different run, export its recorded `RUN` before running the section.
Do not run two verifications concurrently against `latest`.
Cleanup does not need Launch's job table or `wait`.

## Launch

Screen Recording must be granted to the host that runs `screencapture`.
Rust comes from Homebrew rustup.
Each run owns a short scratch path and its process group.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=$(uuidgen | cut -c1-8 | tr A-Z a-z)
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
test ! -e "$DATA" && test ! -e "$PROOF"
cargo build --workspace
mkdir -p "$PROOF"
printf '%s\n' "$RUN" >target/verify-veyra/latest
printf 'RUN=%s\n' "$RUN"
set -m
VEYRA_DATA_DIR="$DATA" target/debug/veyra >"$PROOF/stdout.log" 2>"$PROOF/stderr.log" &
PID=$!
PGID=$(ps -o pgid= -p "$PID" | tr -d ' ')
printf '%s\n' "$PID" >"$PROOF/pid"
printf '%s\n' "$PGID" >"$PROOF/pgid"
test "$PID" = "$PGID"
for attempt in $(seq 1 100); do
    grep -q "^veyra ready pid=$PID data_dir=$DATA$" "$PROOF/stderr.log" && break
    kill -0 "$PID" 2>/dev/null || break
    sleep .1
done
grep -q "^veyra ready pid=$PID data_dir=$DATA$" "$PROOF/stderr.log"
ID=$(swift .agents/skills/verify-veyra/bin/winid.swift "$PID")
test -n "$ID"
printf '%s\n' "$ID" >"$PROOF/window-id"
printf 'PASS launch RUN=%s PID=%s PGID=%s ID=%s\n' "$RUN" "$PID" "$PGID" "$ID"
```

The block must exit 0.
If launch fails after starting a process, run Cleanup before reporting.
Do not use the default data folder.
M0 runs no provider and needs no login or database.

## Doctor

Run Doctor first whenever the instance looks wrong.
Do not drive an instance whose pid, revision, data folder, or window does not match this run.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:-$(<target/verify-veyra/latest)}
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
ID=$(<"$PROOF/window-id")
kill -0 "$PID"
test "$(ps -o pgid= -p "$PID" | tr -d ' ')" = "$PGID"
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl dump >"$PROOF/doctor.json"
python3 - "$PROOF/doctor.json" "$DATA" "$PID" <<'PY'
import json, pathlib, subprocess, sys
dump = json.load(open(sys.argv[1]))
assert dump["data_dir"] == sys.argv[2]
assert dump["pid"] == int(sys.argv[3])
assert pathlib.Path(sys.argv[2], "veyra.lock").read_text().strip() == sys.argv[3]
assert dump["build_rev"] == subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
print("PASS doctor identity")
PY
test -n "$ID"
test "$(swift .agents/skills/verify-veyra/bin/winid.swift "$PID")" = "$ID"
swift .agents/skills/verify-veyra/bin/winid.swift "$PID" --bounds >"$PROOF/bounds.txt"
printf 'PASS doctor window ID=%s\n' "$ID"
```

The client has a two-second read timeout.
A wrong identity or unavailable window fails Doctor.
A build outside Git reports `unknown` instead of HEAD.

## Drive

Run the full commands in [app-launch](features/app-launch.md), including its release check.
The recipe recovers its own run records in a fresh shell.
`action NAME [JSON]` builds a registered gpui action.
`keys KEYSTROKES` dispatches each space-separated key through the window.
Both paths are the same dispatch paths the window uses.
M0's `text STRING` returns an error because its composer has no input handler.
`quit` dispatches the menu's Quit action.

M0's live `dump` fields are `pid`, `build_rev`, `data_dir`, `appearance`, `sidebar.width`, and `notice`.
They report the process, theme, and entity state.
`view` is fixed to `new_thread`, `sidebar.rows` to `[]`, and `composer_text` to an empty string until later milestones.
Do not use those fixed fields as proof of the rendered window or an action.
A screenshot proves the empty sidebar and composer shell.

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
set -eu
RUN=${RUN:-$(<target/verify-veyra/latest)}
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
ID=$(<"$PROOF/window-id")
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl dump >"$PROOF/dump-after.json"
screencapture -x -o -l "$ID" "$PROOF/window.png"
cp "$DATA/logs/veyra.log" "$PROOF/veyra.log"
{ git rev-parse HEAD; git status --short; } >"$PROOF/git-identity.txt"
grep -q "veyra ready pid=$PID data_dir=$DATA" "$PROOF/veyra.log"
for file in dump-before.json dump-after.json window.png veyra.log second-launch.txt drive.log git-identity.txt; do
    test -s "$PROOF/$file"
done
printf 'PASS evidence RUN=%s\n' "$RUN"
```

Inspect the PNG itself.
Require the empty 240 px sidebar, 44 px header, question, and composer shell in Geist.
The composer caps at 784 px with a 16 px corner radius.
The requested window is 1280 by 800 px with a 760 by 560 px minimum.
A window manager can override those outer bounds, so use Doctor's recorded bounds.
A dump alone does not prove visible layout or fonts.
M0 has no database or network side effects.
Do not count native typing, pointer dragging, or a manual theme change as passed through a driver action.

## Cleanup

Run cleanup after successful and failed attempts.
Signal only the processes this run created.
Copy the log before removing scratch state.
A failed Launch may have no window record, so Cleanup accepts an empty `ID`.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:-$(<target/verify-veyra/latest)}
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
ID=
test ! -f "$PROOF/window-id" || ID=$(<"$PROOF/window-id")
test "$PID" = "$PGID"
test ! -f "$DATA/logs/veyra.log" || cp "$DATA/logs/veyra.log" "$PROOF/veyra.log"
if kill -0 "$PID" 2>/dev/null; then
    test "$(ps -o pgid= -p "$PID" | tr -d ' ')" = "$PGID"
    VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl quit >"$PROOF/quit.json" 2>"$PROOF/quit.stderr" || true
fi
for attempt in $(seq 1 50); do
    kill -0 "$PID" 2>/dev/null || break
    sleep .1
done
if kill -0 "$PID" 2>/dev/null; then
    kill -KILL -- "-$PGID"
fi
for attempt in $(seq 1 50); do
    kill -0 "$PID" 2>/dev/null || break
    sleep .1
done
if kill -0 "$PID" 2>/dev/null; then
    printf '%s\n' "cleanup failed; keep $DATA for diagnosis" >&2
    exit 1
fi
test "$DATA" = "/tmp/veyra-verify-$RUN"
rm -rf "$DATA"
test ! -e "$DATA"
/bin/ls -la "$PROOF" >"$PROOF/cleanup-listing.txt"
printf 'PASS cleanup PID=%s gone DATA=%s removed PROOF=%s kept\n' "$PID" "$DATA" "$PROOF"
```

Cleanup leaves `target/verify-veyra/$RUN/` intact.
It also works after a failed recipe that never produced a screenshot.
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
