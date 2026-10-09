# App launch

Status: passed, 2026-10-09, d2bc228, none

Veyra opens one window with an empty project sidebar and a new-thread composer shell.
The same data folder accepts only one instance.
The debug CLI identifies the window without external Accessibility input.

## Sub-features

- `launch-window` shows the empty sidebar, Add project, header, question, and composer in Geist.
- `launch-ready` prints `veyra ready pid=PID data_dir=DIR` after the first frame.
- `launch-identity` reports the live pid, revision, data folder, schema_version, appearance, sidebar width, and notice.
- `launch-database` creates migration 1, WAL, eight tables and the Full access default.
- `launch-recovery` keeps the same database inode and all four database reads across quit and crash relaunches.
- `launch-lock` refuses a second process on the same data folder.
- `launch-log` writes the ready line and action notices to `DATA/logs/veyra.log`.
- `launch-controls` reaches shell controls and sidebar resizing through named actions and individual key bindings.
- `launch-release` contains no debug driver, rejects `ctl`, and creates no control socket.
- `launch-quit` closes the window and leaves proof after cleanup.

## How to get to it (user POV)

- Run `cargo run -p veyra` with no argument.
- Choose Add project in the sidebar, or press ⌘⇧O.
- Choose New thread in the header, or press ⌘N.
- Choose Claude Code, Model, High, Ask first, or Send in the composer shell.
- Drag the sidebar divider, or press ⌘⌥Left and ⌘⌥Right.
- Press ⌘⌥0 to reset the sidebar width.
- Press Escape to dismiss a notice.
- Choose Quit Veyra in the native menu, or press ⌘Q.
The driver reaches those gpui paths.
Native mouse and OS key delivery remain manual checks.

## Driving it with veyra ctl

Launch and Doctor must pass first.
Each block runs in its own fresh bash process from the checkout.
Export the `RUN` that Launch printed before each block, as [SKILL.md](../SKILL.md) says.

The live dump fields are `pid`, `build_rev`, `data_dir`, `schema_version`, `appearance`, `sidebar.width`, and `notice`.
`view`, `sidebar.rows`, and `composer_text` are fixed placeholders until later milestones.
No assertion below uses those fixed fields as evidence.
Inspect the window screenshot for the empty sidebar and composer.
The theme header is 44 px.
The sidebar starts at 240 px, clamps to 200 through 448 px, and moves 16 px per resizing key.
The composer caps at 784 px, with a 48 px text area and a 40 px toolbar.
The instance lock retries for 4 s, so the second-launch check allows 10 s.

The first block captures the baseline, bounds the second launch, and checks each action and key.
Every notice key starts from a cleared notice.
New thread and Escape start from a Send notice.
Each resizing key starts from a width its binding must change.
The transcript records commands, replies, errors, and exit codes in `drive.log`.
A failed assertion stops the block with a nonzero exit.
Run Cleanup even when this block fails.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
ID=$(<"$PROOF/window-id")
ctl() {
    local name=$1 rc
    shift
    printf '$ veyra ctl' >>"$PROOF/drive.log"
    printf ' %q' "$@" >>"$PROOF/drive.log"
    printf '\n' >>"$PROOF/drive.log"
    if VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl "$@" >"$PROOF/$name.json" 2>"$PROOF/$name.stderr"; then
        rc=0
    else
        rc=$?
    fi
    cat "$PROOF/$name.json" "$PROOF/$name.stderr" >>"$PROOF/drive.log"
    printf 'exit=%s\n' "$rc" >>"$PROOF/drive.log"
    test "$rc" = 0 || return "$rc"
    case "$1" in
        action|keys)
            python3 - "$PROOF/$name.json" <<'PY'
import json, sys
assert json.load(open(sys.argv[1])) == {"ok": True}
PY
            ;;
    esac
}
notice() {
    python3 - "$PROOF/$1.json" "$2" <<'PY'
import json, sys
actual = json.load(open(sys.argv[1]))["notice"]
expected = None if sys.argv[2] == '<null>' else sys.argv[2]
assert actual == expected, f'{sys.argv[1]} notice expected {expected!r}, got {actual!r}'
print('PASS', sys.argv[1], 'notice', repr(actual))
PY
}
width() {
    python3 - "$PROOF/$1.json" "$2" <<'PY'
import json, sys
actual = json.load(open(sys.argv[1]))["sidebar"]["width"]
assert actual == int(sys.argv[2]), f'{sys.argv[1]} width expected {sys.argv[2]}, got {actual}'
print('PASS', sys.argv[1], 'width', actual)
PY
}
ctl dump-before dump
width dump-before 240
notice dump-before '<null>'
grep -q "^veyra ready pid=$PID data_dir=$DATA$" "$PROOF/stderr.log"
screencapture -x -o -l "$ID" "$PROOF/window-launch.png"
set -m
VEYRA_DATA_DIR="$DATA" target/debug/veyra >"$PROOF/second-launch.txt" 2>&1 &
SECOND=$!
printf '%s\n' "$SECOND" >"$PROOF/second-pid"
for attempt in $(seq 1 100); do
    kill -0 "$SECOND" 2>/dev/null || break
    sleep .1
done
if kill -0 "$SECOND" 2>/dev/null; then
    printf 'timeout after 10 seconds; stopping owned pid %s\n' "$SECOND" >>"$PROOF/second-launch.txt"
    kill -KILL "$SECOND"
fi
if wait "$SECOND"; then rc=0; else rc=$?; fi
printf 'exit=%s\n' "$rc" >>"$PROOF/second-launch.txt"
cat "$PROOF/second-launch.txt"
grep -q 'Veyra is already running' "$PROOF/second-launch.txt"
grep -qx 'exit=1' "$PROOF/second-launch.txt"
ctl doctor-after-second-launch dump
python3 - "$PROOF/doctor-after-second-launch.json" "$PID" "$DATA" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
assert d['pid'] == int(sys.argv[2]) and d['data_dir'] == sys.argv[3]
print('PASS second launch refused; first instance still answers')
PY
while IFS='|' read -r action key expected; do
    ctl "action-$action" action "veyra::$action"
    ctl "dump-action-$action" dump
    notice "dump-action-$action" "$expected"
    ctl "reset-$key" action veyra::NewThread
    ctl "before-$key" dump
    notice "before-$key" '<null>'
    ctl "key-$key" keys "$key"
    ctl "after-$key" dump
    notice "after-$key" "$expected"
done <<'KEYS'
AddProject|secondary-shift-o|Adding projects is not available in M0.
PickProvider|secondary-alt-p|Provider selection is not available in M0.
PickModel|secondary-alt-m|Model selection is not available in M0.
PickEffort|secondary-alt-e|Effort selection is not available in M0.
PickPermission|secondary-alt-a|Permission selection is not available in M0.
Send|secondary-enter|The M0 composer is a shell. Sending is not available.
KEYS
ctl width-320 action veyra::SetSidebarWidth '{"width":320}'
ctl width-320-dump dump
width width-320-dump 320
ctl key-left keys secondary-alt-left
ctl after-left dump
width after-left 304
ctl reset-right action veyra::SetSidebarWidth '{"width":320}'
ctl before-right dump
width before-right 320
ctl key-right keys secondary-alt-right
ctl after-right dump
width after-right 336
ctl width-100 action veyra::SetSidebarWidth '{"width":100}'
ctl width-100-dump dump
width width-100-dump 200
ctl width-900 action veyra::SetSidebarWidth '{"width":900}'
ctl width-900-dump dump
width width-900-dump 448
ctl key-width-reset keys secondary-alt-0
ctl after-width-reset dump
width after-width-reset 240
ctl before-narrow action veyra::SetSidebarWidth '{"width":320}'
ctl narrow-sidebar action veyra::NarrowSidebar
ctl after-narrow-sidebar dump
width after-narrow-sidebar 304
ctl widen-sidebar action veyra::WidenSidebar
ctl after-widen-sidebar dump
width after-widen-sidebar 320
ctl reset-sidebar action veyra::ResetSidebar
ctl after-reset-sidebar dump
width after-reset-sidebar 240
ctl before-dismiss action veyra::Send
ctl dismiss-notice action veyra::DismissNotice
ctl after-dismiss-notice dump
notice after-dismiss-notice '<null>'
if ctl text-hello text hello; then
    printf '%s\n' 'text unexpectedly accepted' >&2
    exit 1
else
    rc=$?
fi
printf 'exit=%s\n' "$rc" >"$PROOF/text-hello.exit"
test "$rc" = 1
grep -q 'the M0 composer is a shell with no text input' "$PROOF/text-hello.json"
ctl reset-new-thread action veyra::Send
ctl before-new-thread-action dump
notice before-new-thread-action 'The M0 composer is a shell. Sending is not available.'
ctl new-thread-action action veyra::NewThread
ctl after-new-thread-action dump
notice after-new-thread-action '<null>'
for key in secondary-n escape; do
    ctl "reset-$key" action veyra::Send
    ctl "before-$key" dump
    notice "before-$key" 'The M0 composer is a shell. Sending is not available.'
    ctl "key-$key" keys "$key"
    ctl "after-$key" dump
    notice "after-$key" '<null>'
done
printf 'PASS app-launch actions and individual keys RUN=%s\n' "$RUN"
```

### Migrated database and relaunch

Evidence in SKILL.md reads migration 1 through `sqlite3 "$DB"`.
This block proves that graceful quit and `kill -9` keep the same database.
It replaces the process and window records before the next Doctor or Evidence.
Each pid comes from this run's proof and must match `DATA/veyra.lock` before any signal.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
DATA=/tmp/veyra-verify-$RUN
DB=$DATA/veyra.db
PROOF=target/verify-veyra/$RUN
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
test "$(<"$DATA/veyra.lock")" = "$PID"
test "$PID" = "$PGID"
database_read() {
    local label=$1
    {
        sqlite3 "$DB" "pragma user_version"
        sqlite3 "$DB" "pragma journal_mode"
        sqlite3 "$DB" ".tables"
        sqlite3 "$DB" "select default_mode from settings"
    } | tee "$PROOF/$label-database.txt"
    stat -f %i "$DB" | tee "$PROOF/$label-inode.txt"
    VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl dump >"$PROOF/$label.json"
    python3 - "$PROOF" "$label" "$PID" "$DATA" <<'PY'
import json, pathlib, sys
proof, label, pid, data = pathlib.Path(sys.argv[1]), *sys.argv[2:]
actual = (proof / f'{label}-database.txt').read_text().split()
expected = ['1', 'wal', 'grants', 'items', 'projects', 'requests', 'settings', 'tasks', 'threads', 'turns',
            'full_access']
assert actual == expected, f'{label} database reads {actual!r}'
dump = json.loads((proof / f'{label}.json').read_text())
assert dump['schema_version'] == 1 and dump['pid'] == int(pid) and dump['data_dir'] == data, dump
print(f'PASS {label} database reads and dump identity')
PY
}
database_read initial
wait_gone() {
    for attempt in $(seq 1 50); do
        kill -0 "$1" 2>/dev/null || return 0
        sleep .1
    done
    printf 'owned pid %s did not exit\n' "$1" >&2
    return 1
}
relaunch() {
    local label=$1
    set -m
    VEYRA_DATA_DIR="$DATA" target/debug/veyra >"$PROOF/$label.stdout" 2>"$PROOF/$label.stderr" &
    PID=$!
    PGID=$(ps -o pgid= -p "$PID" | tr -d ' ')
    printf '%s\n' "$PID" >"$PROOF/pid"
    printf '%s\n' "$PGID" >"$PROOF/pgid"
    test "$PID" = "$PGID"
    for attempt in $(seq 1 100); do
        grep -q "^veyra ready pid=$PID data_dir=$DATA$" "$PROOF/$label.stderr" && break
        kill -0 "$PID" 2>/dev/null || break
        sleep .1
    done
    cat "$PROOF/$label.stderr" >>"$PROOF/stderr.log"
    grep -q "^veyra ready pid=$PID data_dir=$DATA$" "$PROOF/$label.stderr"
    test "$(<"$DATA/veyra.lock")" = "$PID"
    ID=$(swift .agents/skills/verify-veyra/bin/winid.swift "$PID")
    test -n "$ID"
    printf '%s\n' "$ID" >"$PROOF/window-id"
    database_read "$label"
    test "$(<"$PROOF/initial-inode.txt")" = "$(<"$PROOF/$label-inode.txt")"
    printf 'PASS %s PID=%s schema_version=1 database inode kept\n' "$label" "$PID"
}
VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl quit >"$PROOF/relaunch-quit.json"
printf '%s\n' "$PID" >"$PROOF/quit-old-pid"
wait_gone "$PID"
relaunch after-quit
printf '%s\n' "$PID" >"$PROOF/crash-old-pid"
test "$(<"$DATA/veyra.lock")" = "$PID"
kill -9 "$PID"
wait_gone "$PID"
relaunch after-crash
cp "$PROOF/initial-database.txt" "$PROOF/database-initial.txt"
cp "$PROOF/after-crash-database.txt" "$PROOF/relaunch-database.txt"
```

Run Doctor again for the final pid.

### Release check

Criterion 4 checks the real release binary and an isolated running release window.
The string scan uses Apple's `strings` and the debug binary as a positive control.
The block stops only its own release process group with SIGTERM and a bounded wait.
The debug instance stays up for Evidence and Cleanup.
A failure still runs the release cleanup trap.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
REL_DATA=/tmp/veyra-rel-$RUN
REL_PROOF=$PROOF/release
test ! -e "$REL_DATA"
test ! -e "$REL_PROOF"
mkdir -p "$REL_PROOF"
cargo build --release
if target/release/veyra ctl dump >"$REL_PROOF/ctl.txt" 2>&1; then rc=0; else rc=$?; fi
printf 'exit=%s\n' "$rc" >>"$REL_PROOF/ctl.txt"
cat "$REL_PROOF/ctl.txt"
grep -qx 'usage: veyra \[mcp\]' "$REL_PROOF/ctl.txt"
test "$rc" = 2
RELEASE_COUNT=$(/usr/bin/strings -a target/release/veyra | grep -c control.sock || true)
DEBUG_COUNT=$(/usr/bin/strings -a target/debug/veyra | grep -c control.sock || true)
printf 'release control.sock count=%s\ndebug control.sock count=%s\n' "$RELEASE_COUNT" "$DEBUG_COUNT" | tee "$REL_PROOF/strings.txt"
test "$RELEASE_COUNT" = 0
test "$DEBUG_COUNT" -gt 0
set -m
VEYRA_DATA_DIR="$REL_DATA" target/release/veyra >"$REL_PROOF/stdout.log" 2>"$REL_PROOF/stderr.log" &
REL_PID=$!
REL_PGID=$(ps -o pgid= -p "$REL_PID" | tr -d ' ')
printf '%s\n' "$REL_PID" >"$REL_PROOF/pid"
printf '%s\n' "$REL_PGID" >"$REL_PROOF/pgid"
stop_release() {
    if kill -0 "$REL_PID" 2>/dev/null; then
        test "$REL_PID" = "$REL_PGID" || return 1
        kill -TERM -- "-$REL_PGID"
    fi
    for attempt in $(seq 1 50); do
        kill -0 "$REL_PID" 2>/dev/null || break
        sleep .1
    done
    if kill -0 "$REL_PID" 2>/dev/null; then
        kill -KILL -- "-$REL_PGID"
    fi
    for attempt in $(seq 1 50); do
        kill -0 "$REL_PID" 2>/dev/null || break
        sleep .1
    done
    if kill -0 "$REL_PID" 2>/dev/null; then
        printf 'release cleanup failed; keep %s\n' "$REL_DATA" >&2
        return 1
    fi
    test ! -f "$REL_DATA/logs/veyra.log" || cp "$REL_DATA/logs/veyra.log" "$REL_PROOF/veyra.log"
    test "$REL_DATA" = "/tmp/veyra-rel-$RUN" || return 1
    rm -rf "$REL_DATA"
    test ! -e "$REL_DATA"
    printf 'PASS release cleanup PID=%s gone DATA=%s removed\n' "$REL_PID" "$REL_DATA"
}
trap stop_release EXIT
test "$REL_PID" = "$REL_PGID"
for attempt in $(seq 1 100); do
    grep -q "^veyra ready pid=$REL_PID data_dir=$REL_DATA$" "$REL_PROOF/stderr.log" && break
    kill -0 "$REL_PID" 2>/dev/null || break
    sleep .1
done
grep -q "^veyra ready pid=$REL_PID data_dir=$REL_DATA$" "$REL_PROOF/stderr.log"
/bin/ls -laR "$REL_DATA" | tee "$REL_PROOF/data-listing.txt"
test ! -e "$REL_DATA/control.sock"
printf 'PASS release usage exit=2 control.sock absent RUN=%s\n' "$RUN"
stop_release
trap - EXIT
```

Run Evidence and then Cleanup from [verify-veyra](../SKILL.md).
Require `dump-after.json`, `window.png`, `veyra.log`, and `second-launch.txt` after cleanup.
The log's ready line must name this pid and data folder.
`ctl quit` uses the same Quit action as the menu.
After Cleanup, `/bin/ls "$PROOF"` must still list the proof and `test ! -e "$DATA"` must pass.

## Observable success

- `launch-window` requires `window-launch.png` and `window.png` inspected for the empty sidebar and composer in Geist.
Doctor's bounds show the actual window size.
- `launch-ready` requires the exact pid and data folder in `stderr.log` and `veyra.log`.
- `launch-identity` requires `dump.pid` equal to `DATA/veyra.lock`, `build_rev` equal to Git HEAD, and `data_dir` equal to DATA.
Doctor times the live dump under two seconds.
`appearance`, `sidebar.width` and `notice` are live window state.
- `launch-database` requires `dump.schema_version` and `sqlite3 "$DB" "pragma user_version"` equal to 1.
`pragma journal_mode` prints `wal`.
`.tables` lists `grants items projects requests settings tasks threads turns`.
`select default_mode from settings` prints `full_access`.
- `launch-recovery` requires `after-quit.stderr` and `after-crash.stderr` to reach the ready line on the same DATA.
`initial-database.txt`, `after-quit-database.txt` and `after-crash-database.txt` each assert all four reads.
Their matching inode files require the same database file across both relaunches.
Each saved dump asserts schema version, pid and DATA.
`relaunch-database.txt` keeps the final reads.
- `launch-lock` requires second-launch exit 1 and the already-running message.
`doctor-after-second-launch.json` still reports the first pid and DATA.
- `launch-log` requires ready and action notices in `veyra.log`.
- `launch-controls` requires each `notice` and `sidebar.width` transition in `drive.log`.
This includes `NarrowSidebar`, `WidenSidebar`, `ResetSidebar`, `DismissNotice`, their keys, the shell actions and `SetSidebarWidth`.
`text` must return its no-input-handler error.
- `launch-release` requires `release/ctl.txt` exit 2, zero release driver strings with a positive debug control, and no release control socket.
- `launch-quit` requires every owned pid absent and DATA removed, with the PNG, dumps, database reads and log still in PROOF.

## Gotchas

- The window is still the M0 shell in M1.
Add project, pickers, and Send show notices.
They do not create projects, threads, or provider sessions.
- The composer has no text input through M1.
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
