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
Launch prints `RUN=` followed by the run's identifier.
Export that `RUN` before every later section.
Each section refuses to start without it.
This keeps the section on the instance Launch started.
To resume an earlier run, export the `RUN` Launch printed.
Each section reads the process and window records from the run's proof folder.
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
test ! -e "$DATA"
test ! -e "$PROOF"
cargo build --workspace
mkdir -p "$PROOF"
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
The shell runs no provider and needs no login.
M1 starts the engine and migrates the database before opening the window.

## Doctor

Run Doctor first whenever the instance looks wrong.
Do not drive an instance whose pid, revision, data folder, or window does not match this run.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
ID=$(<"$PROOF/window-id")
kill -0 "$PID"
test "$(ps -o pgid= -p "$PID" | tr -d ' ')" = "$PGID"
python3 - "$PROOF" "$DATA" "$PID" <<'PY'
import json, os, pathlib, subprocess, sys, time
proof = pathlib.Path(sys.argv[1])
start = time.monotonic()
reply = subprocess.run(["target/debug/veyra", "ctl", "dump"], env={**os.environ, "VEYRA_DATA_DIR": sys.argv[2]},
                       stdout=subprocess.PIPE, text=True, check=True).stdout
seconds = time.monotonic() - start
(proof / "doctor.json").write_text(reply)
(proof / "doctor-time.txt").write_text(f"{seconds:.3f}\n")
assert seconds < 2, seconds
print(f"PASS doctor dump answered in {seconds:.3f} s")
dump = json.loads(reply)
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

Doctor times the dump and requires an answer within two seconds.
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

The live `dump` fields are `pid`, `build_rev`, `data_dir`, `schema_version`, `appearance`, `sidebar.width`, and `notice`.
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

Run this block only after both relaunches, the final Doctor, and Release check.
A missing relaunch or release artifact fails the block.
Capture the action and its resulting state.
Keep dump JSON before and after, the ready line, second-launch stderr and exit code, the app log, and the window.
Use the pid filter so another Veyra window cannot supply the screenshot.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
DATA=/tmp/veyra-verify-$RUN
DB=$DATA/veyra.db
PROOF=target/verify-veyra/$RUN
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
ID=$(<"$PROOF/window-id")
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl dump >"$PROOF/dump-after.json"
{
    sqlite3 "$DB" "pragma user_version"
    sqlite3 "$DB" "pragma journal_mode"
    sqlite3 "$DB" ".tables"
    sqlite3 "$DB" "select default_mode from settings"
} | tee "$PROOF/database.txt"
test "$(sqlite3 "$DB" "pragma user_version")" = 1
test "$(sqlite3 "$DB" "pragma journal_mode")" = wal
test "$(sqlite3 "$DB" "select default_mode from settings")" = full_access
python3 - "$DB" "$PROOF/dump-after.json" <<'PY'
import json, subprocess, sys
assert json.load(open(sys.argv[2]))['schema_version'] == 1
assert subprocess.check_output(['sqlite3', sys.argv[1], '.tables'], text=True).split() == [
    'grants', 'items', 'projects', 'requests', 'settings', 'tasks', 'threads', 'turns']
print('PASS migrated database and schema_version')
PY
screencapture -x -o -l "$ID" "$PROOF/window.png"
cp "$DATA/logs/veyra.log" "$PROOF/veyra.log"
{ git rev-parse HEAD; git status --short; } >"$PROOF/git-identity.txt"
grep -q "veyra ready pid=$PID data_dir=$DATA" "$PROOF/veyra.log"
for file in dump-before.json dump-after.json window.png veyra.log second-launch.txt drive.log \
    git-identity.txt database.txt initial-database.txt initial-inode.txt initial.json \
    after-quit.stderr after-quit.json after-quit-database.txt after-quit-inode.txt \
    after-crash.stderr after-crash.json after-crash-database.txt after-crash-inode.txt \
    relaunch-database.txt release/ctl.txt release/strings.txt release/data-listing.txt release/stderr.log; do
    test -s "$PROOF/$file" || { printf 'missing proof %s\n' "$file" >&2; exit 1; }
done
python3 - "$PROOF" "$DATA" <<'PY'
import json, pathlib, re, sys
proof, data = pathlib.Path(sys.argv[1]), sys.argv[2]
recipe = pathlib.Path('.agents/skills/verify-veyra/features/app-launch.md').read_text()
keys = recipe.split("done <<'KEYS'\n", 1)[1].split('\nKEYS', 1)[0]
log = (proof / 'veyra.log').read_text()
for row in keys.splitlines():
    message = row.split('|', 2)[2]
    assert f'shell action {message}' in log, f'missing log notice {message!r}'
print('PASS all driven action notices logged')
expected = ['1', 'wal', 'grants', 'items', 'projects', 'requests', 'settings', 'tasks', 'threads', 'turns',
            'full_access']
inodes = []
for label in ['initial', 'after-quit', 'after-crash']:
    actual = (proof / f'{label}-database.txt').read_text().split()
    assert actual == expected, f'{label} database reads {actual!r}'
    inodes.append((proof / f'{label}-inode.txt').read_text().strip())
    dump = json.loads((proof / f'{label}.json').read_text())
    assert dump['schema_version'] == 1 and dump['data_dir'] == data, dump
    ready = f'veyra ready pid={dump["pid"]} data_dir={data}'
    assert re.search(rf'^{re.escape(ready)}$', (proof / 'stderr.log').read_text(), re.M), ready
assert len(set(inodes)) == 1 and inodes[0].isdigit(), f'database inodes changed {inodes!r}'
print('PASS relaunch reads, identities and database inode kept')
assert re.search(r'^exit=2$', (proof / 'release/ctl.txt').read_text(), re.M)
strings = (proof / 'release/strings.txt').read_text()
assert re.search(r'^release control.sock count=0$', strings, re.M)
assert int(re.search(r'^debug control.sock count=(\d+)$', strings, re.M)[1]) > 0
assert 'control.sock' not in (proof / 'release/data-listing.txt').read_text()
print('PASS release proof kept')
PY
printf 'PASS evidence RUN=%s\n' "$RUN"
```

Inspect the PNG itself.
Require the empty 240 px sidebar, 44 px header, question, and composer shell in Geist.
The composer caps at 784 px with a 16 px corner radius.
The requested window is 1280 by 800 px with a 760 by 560 px minimum.
A window manager can override those outer bounds, so use Doctor's recorded bounds.
A dump alone does not prove visible layout or fonts.
M1 creates `veyra.db` with migration 1 and no provider or network side effects.
`database.txt` keeps the schema version, journal mode, table names and default permission mode.
Evidence also rereads all saved relaunch snapshots, matching database inodes, driven log notices and release results.
M1 offers no driver command that writes a persistent value before relaunch.
The inode check proves file preservation without adding a database setter.
Do not count native typing, pointer dragging, or a manual theme change as passed through a driver action.

## Cleanup

Run cleanup after successful and failed attempts.
Signal only the processes this run created.
Copy the log before removing scratch state.
A failed Launch may have no window record, so Cleanup accepts an empty `ID`.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
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
