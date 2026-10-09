# App launch

Status: passed, 2026-10-09, d2bc228, none

## Sub-features

- `launch-window`: one native window, Geist, sidebar, header and upper-third new-thread composer.
- `launch-ready`: exact ready line with this run's pid and data folder.
- `launch-identity`: live pid, build revision, data folder, schema_version and system appearance.
- `launch-database`: migration 2, WAL, eight tables and Full access default.
- `launch-recovery`: the project written before quit survives quit and crash relaunches in the same database.
- `launch-lock`: a second process refuses the same folder while the first still answers.
- `launch-controls`: live width and notice transitions through actions and individual keys.
- `launch-log`: the ready line and rejected Send notice reach the application log.
- `launch-release`: no driver or provider override strings, ctl rejected, no release socket.
- `launch-quit`: owned processes gone, scratch removed and proof kept.

## How to get to it (user POV)

Launch Veyra normally.
Add project opens a folder picker; ⌘⇧O reaches `AddProject`.
New thread or ⌘N reaches `NewThread`.
Drag the divider, or use ⌘⌥Left, ⌘⌥Right and ⌘⌥0.
Escape dismisses a notice through `DismissNotice`.
The menu's Quit Veyra and ⌘Q reach `Quit`.
Native pointer delivery is a manual check.
The driver supplies an AddProject path to the same action after a user would pick a folder.

## Driving it with veyra ctl

Run Launch and Doctor from [SKILL.md](../SKILL.md) first.
Export its RUN into this fresh shell.
The second-launch check allows 10 seconds because the instance lock retries for about four.
No provider turn runs in this page.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
pid = identity()
dump = ctl('dump-before','dump')
assert dump['sidebar']['width'] == 240 and dump['sidebar']['rows'] == []
assert dump['notice'] is None and dump['view'] == 'new_thread'
capture('window-launch')
start = time.monotonic()
second = subprocess.Popen(['target/debug/veyra'],env=ENV,stdout=(PROOF/'second-launch.txt').open('w'),
    stderr=subprocess.STDOUT,start_new_session=True)
(PROOF/'second-pid').write_text(str(second.pid)+'\n')
try: code = second.wait(timeout=10)
except subprocess.TimeoutExpired:
    os.killpg(second.pid,signal.SIGKILL)
    second.wait()
    raise AssertionError('second launch exceeded 10 seconds')
with (PROOF/'second-launch.txt').open('a') as out: out.write(f'exit={code}\n')
assert code == 1 and 'Veyra is already running' in (PROOF/'second-launch.txt').read_text()
assert ctl('doctor-after-second-launch','dump')['pid'] == pid
print(f'PASS second launch refused in {time.monotonic()-start:.3f}s; first pid still answers')
sizes = [('NarrowSidebar','secondary-alt-left',320,304),('WidenSidebar','secondary-alt-right',320,336),
    ('ResetSidebar','secondary-alt-0',320,240)]
for operation,key,before,after in sizes:
    action(operation+'-baseline','SetSidebarWidth',{'width':before})
    action(operation+'-action',operation)
    assert ctl(operation+'-after-action','dump')['sidebar']['width'] == after
    action(operation+'-key-baseline','SetSidebarWidth',{'width':before})
    ctl(operation+'-key','keys',key)
    assert ctl(operation+'-after-key','dump')['sidebar']['width'] == after
for width,expected in [(100,200),(900,448)]:
    action('clamp-'+str(width),'SetSidebarWidth',{'width':width})
    assert ctl('clamp-result-'+str(width),'dump')['sidebar']['width'] == expected
for operation,key in [('NewThread','secondary-n'),('DismissNotice','escape')]:
    for kind in ['action','key']:
        action(operation+'-'+kind+'-notice','Send')
        assert ctl(operation+'-'+kind+'-notice-dump','dump')['notice'] == 'Add a project to start a thread'
        if kind == 'action': action(operation+'-'+kind,operation)
        else: ctl(operation+'-'+kind,'keys',key)
        assert ctl(operation+'-'+kind+'-after','dump')['notice'] is None
assert ctl('text-hello','text','hello') == {'ok':True}
assert ctl('text-hello-after','dump')['composer_text'] == 'hello'
action('reset-new-thread','NewThread')
assert ctl('text-reset','dump')['composer_text'] == ''
action('reset-sidebar','ResetSidebar')
log = (DATA/'logs/veyra.log').read_text()
assert f'veyra ready pid={pid} data_dir={DATA}' in log
assert 'window notice' in log and 'Add a project to start a thread' in log
print('PASS app-launch ready line and rejected Send notice logged')
print('PASS app-launch actions, individual keys, input handler and width clamps')
PY
```

### Migrated database and relaunch

The project write is the pre-quit value.
The helper uses one expected table list in SKILL.md.
A missing or lost WAL row fails the project equality after either relaunch.
Relaunch updates the pid, group and window records before Doctor or Evidence.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
identity()
action('persistent-project','AddProject',{'path':str(DATA/'project')})
poll('persistent-project-added',lambda d: any(row['kind']=='project' for row in d['sidebar']['rows']))
row = sql('select id,path,name from projects order by id')
assert row
(PROOF/'persisted-project.txt').write_text(row+'\n')
database('initial','full_access')
for label,crash in [('after-quit',False),('after-crash',True)]:
    restart(label,crash)
    database(label,'full_access')
    assert sql('select id,path,name from projects order by id') == row, label
    assert (PROOF/(label+'-inode.txt')).read_text() == (PROOF/'initial-inode.txt').read_text(), label
    print(f'PASS {label} project row, migration, WAL and inode kept')
page_done('app-launch')
PY
```

### Release check

Use a disposable HOME and a PATH with no Claude to prevent an unauthorized live probe.
Require the known system-wide install paths to contain no Claude before launch.
Release still reads VEYRA_DATA_DIR, but excludes the driver, recording and provider overrides at compile time.
The debug strings are the positive control.
The cleanup trap owns only this release group and scratch folder.
This section stops the debug app before opening the release window, then relaunches debug after release cleanup.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
REL_DATA=/tmp/veyra-rel-$RUN
REL_PROOF=$PROOF/release
test ! -e "$REL_DATA"
mkdir -p "$REL_PROOF"
cargo build --release -p veyra
if target/release/veyra ctl dump >"$REL_PROOF/ctl.txt" 2>&1; then rc=0; else rc=$?; fi
printf 'exit=%s\n' "$rc" >>"$REL_PROOF/ctl.txt"
test "$rc" = 2
grep -qx 'usage: veyra \[mcp\]' "$REL_PROOF/ctl.txt"
: >"$REL_PROOF/strings.txt"
for name in control.sock VEYRA_CLAUDE_BIN VEYRA_FAKE_FIXTURES VEYRA_RECORD; do
    release=$(/usr/bin/strings -a target/release/veyra | grep -c "$name" || true)
    debug=$(/usr/bin/strings -a target/debug/veyra | grep -c "$name" || true)
    printf 'release %s count=%s\ndebug %s count=%s\n' "$name" "$release" "$name" "$debug" \
        | tee -a "$REL_PROOF/strings.txt"
    test "$release" = 0
    test "$debug" -gt 0
done
for path in /opt/homebrew/bin/claude /usr/local/bin/claude /usr/bin/claude; do test ! -x "$path"; done
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
stop('before-release')
PY
DEBUG_PID=$(<"$PROOF/pid")
if kill -0 "$DEBUG_PID" 2>/dev/null; then
    printf 'debug app is still running before release launch\n' >&2
    exit 1
fi
mkdir -p "$REL_DATA/home"
set -m
HOME="$REL_DATA/home" SHELL=/usr/bin/false PATH=/usr/bin:/bin:/usr/sbin:/sbin VEYRA_DATA_DIR="$REL_DATA" \
    target/release/veyra >"$REL_PROOF/stdout.log" 2>"$REL_PROOF/stderr.log" &
REL_PID=$!
REL_PGID=$(ps -o pgid= -p "$REL_PID" | tr -d ' ')
printf '%s\n' "$REL_PID" >"$REL_PROOF/pid"
printf '%s\n' "$REL_PGID" >"$REL_PROOF/pgid"
stop_release() {
    if kill -0 "$REL_PID" 2>/dev/null; then
        test "$REL_PID" = "$REL_PGID" || return 1
        kill -TERM -- "-$REL_PGID"
    fi
    for attempt in $(seq 1 50); do kill -0 "$REL_PID" 2>/dev/null || break; sleep .1; done
    if kill -0 "$REL_PID" 2>/dev/null; then kill -KILL -- "-$REL_PGID"; fi
    for attempt in $(seq 1 50); do kill -0 "$REL_PID" 2>/dev/null || break; sleep .1; done
    ! kill -0 "$REL_PID" 2>/dev/null || return 1
    test ! -f "$REL_DATA/logs/veyra.log" || cp "$REL_DATA/logs/veyra.log" "$REL_PROOF/veyra.log"
    test "$REL_DATA" = "/tmp/veyra-rel-$RUN" || return 1
    rm -rf "$REL_DATA"
    printf 'PASS release cleanup PID=%s DATA=%s\n' "$REL_PID" "$REL_DATA"
}
trap stop_release EXIT
test "$REL_PID" = "$REL_PGID"
for attempt in $(seq 1 100); do
    grep -q "^veyra ready pid=$REL_PID data_dir=$REL_DATA$" "$REL_PROOF/stderr.log" && break
    kill -0 "$REL_PID" 2>/dev/null || break
    sleep .1
done
grep -q "^veyra ready pid=$REL_PID data_dir=$REL_DATA$" "$REL_PROOF/stderr.log"
/bin/ls -laR "$REL_DATA" >"$REL_PROOF/data-listing.txt"
test ! -e "$REL_DATA/control.sock"
printf 'PASS release usage exit=2; driver and override strings absent; no socket\n'
stop_release
trap - EXIT
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
start('after-release')
print('PASS debug relaunched after release window ended')
PY
```

## Observable success

- Window: inspect `window-launch.png` for the empty sidebar, left-aligned Add project, header, upper-third composer and Geist.
- Identity: Doctor asserts the live pid, revision, data folder, schema_version, system appearance and window id, within two seconds.
- Lock: `second-launch.txt` contains the refusal and exit 1; the first process still answers.
- Controls: `drive.log` records exit codes and all asserted width, text and notice transitions.
- Log: `veyra.log` retains the asserted ready line and rejected Send notice.
- Recovery: `persisted-project.txt` matches the SQLite row after both relaunches; migration, WAL, tables, mode and inode are asserted every time.
- Release: `release/strings.txt` has four zero release counts and four positive debug controls, `ctl.txt` exits 2 and `data-listing.txt` has no socket.
- Cleanup: `marker-check.txt` passes and DATA is absent while proof remains.

## Gotchas

The composer is now real; a text error is not expected while it is focused.
Adding a project and checking readiness run no turn.
This page leaves the fixture binary missing for new-thread's installation proof.
The native folder picker, pointer resizing and theme switching remain manual entry points.
Do not launch a release check that can find the user's Claude without consent.
