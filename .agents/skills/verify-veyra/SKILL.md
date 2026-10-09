---
name: verify-veyra
description: Verify Veyra's native macOS window and CLI after UI, launch, logging, data-folder, or debug-driver changes. Launch an isolated instance, drive gpui actions through veyra ctl, capture the window, and preserve evidence after cleanup.
---

# Verify Veyra

Read [the feature map](features/README.md) before choosing a recipe.
Run every section in a fresh `bash -c` from this checkout.
Launch prints `RUN=`; export that `RUN` into every later shell.
Each section recovers its records from `target/verify-veyra/$RUN`.
Never drive an instance this run did not start.
Screen Recording must be granted to the host running `screencapture`.
External Accessibility input is not granted here; driver keys prove gpui dispatch, not native keyboard delivery.

## Launch

The default verification lane uses only the fake CLI.
Its binary path deliberately starts missing, so [new-thread](features/new-thread.md) can prove installation and Check again.
The launch writes a small driver helper into the proof folder so the feature pages share logging, bounded polling and the one expected database-table list.
The helper does not set application state; its writes are fixture copies, screenshots and proof.
A live lane must explicitly replace these overrides and have the human's consent.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=$(uuidgen | cut -c1-8 | tr A-Z a-z)
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
test ! -e "$DATA"
test ! -e "$PROOF"
cargo build --workspace
mkdir -p "$PROOF/fixtures" "$DATA/project"
cp crates/engine/tests/fixtures/claude/probe-initialize.jsonl "$PROOF/fixtures/probe.jsonl"
printf 'RUN=%s\n' "$RUN"
cat >"$PROOF/driver.py" <<'PY'
import json, os, pathlib, shutil, signal, sqlite3, subprocess, time
PROOF = pathlib.Path(__file__).resolve().parent
RUN = os.environ['RUN']
DATA = pathlib.Path('/tmp/veyra-verify-' + RUN)
DB = DATA / 'veyra.db'
FIXTURES = PROOF / 'fixtures'
SOURCE = pathlib.Path('crates/engine/tests/fixtures/claude')
TABLES = 'grants items projects requests settings tasks threads turns'.split()
ENV = {**os.environ, 'VEYRA_DATA_DIR': str(DATA), 'VEYRA_CLAUDE_BIN': str(DATA/'fake-cli'),
       'VEYRA_FAKE_FIXTURES': str(FIXTURES)}
def save(name, value):
    (PROOF / (name + '.json')).write_text(json.dumps(value, indent=2) + '\n')
    return value
def ctl(name, *args, expected=0):
    result = subprocess.run(['target/debug/veyra', 'ctl', *args], env=ENV, capture_output=True, text=True)
    with (PROOF/'drive.log').open('a') as log:
        log.write('$ veyra ctl ' + ' '.join(args) + '\n' + result.stdout + result.stderr + f'exit={result.returncode}\n')
    (PROOF/(name+'.json')).write_text(result.stdout)
    (PROOF/(name+'.stderr')).write_text(result.stderr)
    assert result.returncode == expected, (name, expected, result.returncode, result.stdout, result.stderr)
    return json.loads(result.stdout) if result.stdout.startswith('{') else None
def poll(name, predicate):
    end = time.monotonic() + 30
    while time.monotonic() < end:
        dump = ctl(name, 'dump')
        if predicate(dump): return dump
        time.sleep(.05)
    raise AssertionError(name + ' timed out')
def action(name, operation, data=None):
    args = ['action', 'veyra::' + operation]
    if data is not None: args.append(json.dumps(data))
    assert ctl(name, *args) == {'ok': True}
def identity():
    pid = int((PROOF/'pid').read_text())
    pgid = int((PROOF/'pgid').read_text())
    assert os.getpgid(pid) == pgid == pid
    assert (DATA/'veyra.lock').read_text().strip() == str(pid)
    dump = ctl('identity', 'dump')
    assert dump['pid'] == pid and dump['data_dir'] == str(DATA)
    assert dump['build_rev'] == subprocess.check_output(['git','rev-parse','HEAD'], text=True).strip()
    return pid
def capture(name):
    subprocess.run(['screencapture','-x','-o','-l',(PROOF/'window-id').read_text().strip(),str(PROOF/(name+'.png'))],check=True)
def sql(query):
    return subprocess.check_output(['sqlite3',str(DB),query],text=True).strip()
def database(label, mode):
    values = [sql('pragma user_version'),sql('pragma journal_mode'),*sql('.tables').split(),sql('select default_mode from settings')]
    assert values == ['1','wal',*TABLES,mode], (label,values)
    (PROOF/(label+'-database.txt')).write_text('\n'.join(values)+'\n')
    (PROOF/(label+'-inode.txt')).write_text(str(DB.stat().st_ino)+'\n')
    save(label,ctl(label+'-dump','dump'))
def wait_gone(pid):
    end = time.monotonic() + 5
    while time.monotonic() < end:
        try: os.waitpid(pid,os.WNOHANG)
        except ChildProcessError: pass
        try: os.kill(pid,0)
        except ProcessLookupError: return
        time.sleep(.05)
    raise AssertionError(f'owned pid {pid} did not exit')
def restart(label, crash=False):
    pid = identity()
    assert sql('select count(*) from threads where live_pgid is not null') == '0', 'finish provider turns before relaunch'
    if crash: os.kill(pid,signal.SIGKILL)
    else: ctl(label+'-quit','quit')
    wait_gone(pid)
    app = subprocess.Popen(['target/debug/veyra'],env=ENV,stdout=(PROOF/(label+'.stdout')).open('w'),stderr=(PROOF/(label+'.stderr')).open('w'),start_new_session=True)
    pid = app.pid
    (PROOF/'pid').write_text(str(pid)+'\n')
    (PROOF/'pgid').write_text(str(os.getpgid(pid))+'\n')
    with (PROOF/'owned-app-pids').open('a') as out: out.write(str(pid)+'\n')
    end = time.monotonic() + 30
    ready = f'veyra ready pid={pid} data_dir={DATA}'
    while time.monotonic() < end:
        assert app.poll() is None, (label,app.returncode)
        if ready in (PROOF/(label+'.stderr')).read_text(): break
        time.sleep(.05)
    assert ready in (PROOF/(label+'.stderr')).read_text()
    with (PROOF/'stderr.log').open('a') as out: out.write((PROOF/(label+'.stderr')).read_text())
    window = subprocess.check_output(['swift','.agents/skills/verify-veyra/bin/winid.swift',str(pid)],text=True).strip()
    assert window
    (PROOF/'window-id').write_text(window+'\n')
    identity()
    return pid
def stage(scenario, slow=False):
    records = [json.loads(line) for line in (SOURCE/(scenario+'.jsonl')).read_text().splitlines()]
    if slow:
        expanded = []
        for record in records:
            wire = record.get('line',{})
            if wire.get('event',{}).get('delta',{}).get('text') == 'ok':
                for text in ['o','k']:
                    part = json.loads(json.dumps(record))
                    part['line']['event']['delta']['text'] = text
                    part['after_ms'] = 500
                    expanded.append(part)
            else:
                if wire.get('type') == 'result': record['after_ms'] = 500
                expanded.append(record)
        records = expanded
    text = '\n'.join(json.dumps(record) for record in records)+'\n'
    for n in range(1,int(sql('select count(*) from threads'))+2):
        for epoch in range(1,int(sql('select count(*) from turns'))+2):
            (FIXTURES/f't{n}-s{epoch}.jsonl').write_text(text)
def create(name, prompt, slow=False, scenario='ok-reply'):
    stage(scenario,slow)
    action(name+'-new','NewThread')
    assert ctl(name+'-text','text',prompt) == {'ok':True}
    before = int(sql('select count(*) from threads'))
    ctl(name+'-send','keys','secondary-enter')
    dump = poll(name+'-selected',lambda d: d['selected_thread'] is not None and int(sql('select count(*) from threads')) == before+1)
    return dump['selected_thread']['id']
PY
set -m
VEYRA_DATA_DIR="$DATA" VEYRA_CLAUDE_BIN="$DATA/fake-cli" VEYRA_FAKE_FIXTURES="$PROOF/fixtures" target/debug/veyra >"$PROOF/stdout.log" 2>"$PROOF/stderr.log" &
PID=$!
PGID=$(ps -o pgid= -p "$PID" | tr -d ' ')
printf '%s\n' "$PID" >"$PROOF/pid"
printf '%s\n' "$PGID" >"$PROOF/pgid"
printf '%s\n' "$PID" >"$PROOF/owned-app-pids"
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
A failed launch may still own a process: run Cleanup before reporting.
Relaunches use `start_new_session=True`, preserving the same pid-equals-group invariant without a previous shell's job table.
The slowed `ok` fixture uses 500 ms, the design's stderr-wait bound, between its two text deltas and before the result.
Those delays affect proof copies only; the committed capture-derived fixture is unchanged.

## Doctor

Doctor fails on a wrong process, revision, folder, window or appearance.
The macOS appearance read treats a missing `AppleInterfaceStyle` key as light.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
start = time.monotonic()
pid = identity()
dump = ctl('doctor','dump')
seconds = time.monotonic()-start
(PROOF/'doctor-time.txt').write_text(f'{seconds:.3f}\n')
assert seconds < 2, seconds
style = subprocess.run(['defaults','read','-g','AppleInterfaceStyle'],capture_output=True,text=True)
expected = 'dark' if style.returncode == 0 and style.stdout.strip().lower() == 'dark' else 'light'
assert dump['appearance'] == expected, (dump['appearance'],expected)
end = time.monotonic() + 30
while time.monotonic() < end:
    found = subprocess.run(['swift','.agents/skills/verify-veyra/bin/winid.swift',str(pid)],capture_output=True,text=True)
    if found.returncode == 0: break
    time.sleep(.05)
assert found.returncode == 0, found.stderr
window = found.stdout.strip()
assert window == (PROOF/'window-id').read_text().strip()
bounds = subprocess.check_output(['swift','.agents/skills/verify-veyra/bin/winid.swift',str(pid),'--bounds'],text=True)
(PROOF/'bounds.txt').write_text(bounds)
print(f'PASS doctor pid={pid} appearance={expected} seconds={seconds:.3f} window={window}')
PY
```

## Drive

Run [app-launch](features/app-launch.md), then [new-thread](features/new-thread.md), [chat-send-stream](features/chat-send-stream.md) and [settings](features/settings.md).
Use the same exported `RUN` in every fresh shell.
`action NAME [JSON]` builds a registered gpui action.
Action and key replies acknowledge dispatch; poll `dump` for the resulting view or use `ctl wait` for a thread state.
`keys KEYSTROKES` dispatches each key through the window.
`text STRING` uses the focused textarea's public input handler; Settings has no text handler.
Enter or ⌘↵ sends through `veyra::Send`; Shift+Enter inserts a newline.
`wait THREAD STATUS [SECONDS]` is a client-only loop over `dump`, polling every 50 ms for 30 seconds by default.
It exits 0 for the status, 1 for timeout, 2 for bad usage and 3 for a missing or archived thread.
`quit` dispatches the menu's Quit action.
A timeout does not cancel an enqueued action: run Doctor before retrying.

The read-only dump reports `pid`, `build_rev`, `data_dir`, `schema_version`, `appearance`, `view`, `sidebar`, `selected_thread`, `transcript`, `composer_text`, `composer`, `providers`, `settings`, `perf` and `notice`.
`view` is `new_thread`, `chat`, `failed_thread` or `settings`.
`sidebar.rows` holds project and thread rows, with each thread's own status.
`selected_thread` includes its native id, configuration and failure cause.
`composer` includes the chosen provider and permission mode, plus Send's readiness and disabled reason.
`providers` reports each probe's path, version, models and readiness.
`settings.default_mode` is the stored default; `perf.applied_batches` and `perf.max_batch_gap_ms` report the event pump.
A dump proves state, not layout; inspect each PNG.

## Evidence

Run this after all four feature pages and the final Doctor.
The required files enforce the relaunch, release and feature proofs.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
identity()
ctl('dump-after','dump')
database('final','ask_first')
capture('window')
shutil.copy2(DATA/'logs/veyra.log',PROOF/'veyra.log')
(PROOF/'git-identity.txt').write_text(subprocess.check_output(['git','rev-parse','HEAD'],text=True)+subprocess.check_output(['git','status','--short'],text=True))
for name in ['window-launch.png','dump-before.json','second-launch.txt','drive.log','initial-database.txt',
             'after-quit-database.txt','after-crash-database.txt','persisted-project.txt','release/ctl.txt',
             'release/strings.txt','release/data-listing.txt','new-thread-missing.json','new-thread-ready.json',
             'new-thread-no-account.json','new-thread.png','chat-working.json','chat-working.png','chat-done.json',
             'chat-done.png','chat-failed.json','chat-failed.png','settings-first.json','settings-ask-first.json',
             'settings-relaunch.json','settings.png','settings-existing.json','settings-new.json']:
    assert (PROOF/name).stat().st_size > 0, name
assert len({(PROOF/(label+'-inode.txt')).read_text() for label in ['initial','after-quit','after-crash']}) == 1
for pid in (PROOF/'owned-app-pids').read_text().split():
    assert f'veyra ready pid={pid} data_dir={DATA}' in (PROOF/'veyra.log').read_text()
print('PASS evidence identity, database, relaunches, features and release kept')
PY
```

Inspect the PNGs themselves for Geist, the 240 px initial sidebar, 44 px header, upper-third new-thread block, centred chat column and composer, collapsed tools/reasoning, failure cause and Settings copy.
The transcript caps at 752 px, the composer at 784 px, with 40 px gutters.
Yabai can override the requested 1280 by 800 outer bounds; use Doctor's actual bounds.
Do not count native typing, pointer dragging, IME or a manual theme change as driver proof.

## Cleanup

Read `live_pgid` before quitting or removing anything.
Kill only groups saved in this run's database and the app group this run started.
The marker check uses Darwin's `KERN_PROCARGS2` through `sysctl` in memory, never printing arguments or environment values.
It checks live processes with this run's effective user for `VEYRA_PARENT_PID` naming any app pid this run started.
macOS refuses argument reads for privileged processes whose real uid is the user but effective uid is root; a fake CLI lane cannot create such a process.
Zombies have no argument/environment block on Darwin and are excluded before the read; owned app pids must still be absent.
A failed database read, group check or marker check leaves DATA for diagnosis.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
test "$PID" = "$PGID"
sqlite3 "$DATA/veyra.db" 'select live_pgid from threads where live_pgid is not null' >"$PROOF/provider-groups.txt"
test ! -f "$DATA/logs/veyra.log" || cp "$DATA/logs/veyra.log" "$PROOF/veyra.log"
while read -r group; do
    test -z "$group" || kill -KILL -- "-$group" 2>/dev/null || true
done <"$PROOF/provider-groups.txt"
if kill -0 "$PID" 2>/dev/null; then
    test "$(ps -o pgid= -p "$PID" | tr -d ' ')" = "$PGID"
    VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl quit >"$PROOF/quit.json" 2>"$PROOF/quit.stderr" || true
fi
for attempt in $(seq 1 50); do
    kill -0 "$PID" 2>/dev/null || break
    sleep .1
done
if kill -0 "$PID" 2>/dev/null; then kill -KILL -- "-$PGID"; fi
for attempt in $(seq 1 50); do
    kill -0 "$PID" 2>/dev/null || break
    sleep .1
done
! kill -0 "$PID" 2>/dev/null
python3 - "$PROOF" <<'PY'
import ctypes, errno, os, pathlib, struct, subprocess, sys
proof = pathlib.Path(sys.argv[1])
parents = {int(pid) for pid in (proof/'owned-app-pids').read_text().split()}
groups = {int(group) for group in (proof/'provider-groups.txt').read_text().split()}
for group in groups:
    try: os.killpg(group,0)
    except ProcessLookupError: continue
    raise AssertionError(f'owned provider group {group} still exists')
lib = ctypes.CDLL(None,use_errno=True)
lib.sysctl.argtypes = [ctypes.POINTER(ctypes.c_int),ctypes.c_uint,ctypes.c_void_p,ctypes.POINTER(ctypes.c_size_t),ctypes.c_void_p,ctypes.c_size_t]
def no_environment(pid):
    status = subprocess.run(['ps','-o','stat=','-p',str(pid)],capture_output=True,text=True).stdout.strip()
    return not status or status.startswith('Z')
rows = subprocess.check_output(['ps','-axo','pid=,uid=,stat='],text=True).splitlines()
for row in rows:
    text,uid,status = row.split()
    if int(uid) != os.geteuid() or status.startswith('Z'): continue
    pid = int(text)
    mib = (ctypes.c_int*3)(1,49,pid)
    size = ctypes.c_size_t()
    if lib.sysctl(mib,3,None,ctypes.byref(size),None,0) != 0:
        error = ctypes.get_errno()
        if error == errno.ESRCH or (error == errno.EINVAL and no_environment(pid)): continue
        raise OSError(error,f'marker size read failed for pid {pid}')
    buffer = ctypes.create_string_buffer(size.value)
    if lib.sysctl(mib,3,buffer,ctypes.byref(size),None,0) != 0:
        error = ctypes.get_errno()
        if error == errno.ESRCH or (error == errno.EINVAL and no_environment(pid)): continue
        raise OSError(error,f'marker read failed for pid {pid}')
    raw = buffer.raw[:size.value]
    argc = struct.unpack_from('i',raw)[0]
    offset = raw.index(b'\0',4)+1
    while offset < len(raw) and raw[offset] == 0: offset += 1
    for _ in range(argc): offset = raw.index(b'\0',offset)+1
    markers = [value.split(b'=',1)[1] for value in raw[offset:].split(b'\0') if value.startswith(b'VEYRA_PARENT_PID=')]
    assert not any(value.decode().isdigit() and int(value) in parents for value in markers), f'owned marker remains on pid {pid}'
(proof/'marker-check.txt').write_text(f'PASS no marked processes for owned app pids {sorted(parents)}\n')
print('PASS owned pids, provider groups and marker check')
PY
test "$DATA" = "/tmp/veyra-verify-$RUN"
rm -rf "$DATA"
test ! -e "$DATA"
/bin/ls -la "$PROOF" >"$PROOF/cleanup-listing.txt"
printf 'PASS cleanup PID=%s gone DATA=%s removed PROOF=%s kept\n' "$PID" "$DATA" "$PROOF"
```

Cleanup leaves `target/verify-veyra/$RUN` intact.
If Launch failed before the database exists, record that failure and stop only its recorded app group; do not claim the normal marker/database cleanup passed.
Never kill by name.

## Helpers

`bin/winid.swift PID` finds the on-screen window by owner pid and normal window layer.
Add `--bounds` to get `x,y,w,h`.
A socket path permits 103 usable bytes; keep RUN short.

## Maintain the map

Update the matching feature page when actions, commands or dump fields change.
`verify_map_names` checks exact action names, ctl verbs and top-level dump keys.
The lead owns the `Status: STATE, DATE, COMMIT, MISSING` values and their docs-only update after the phase commit.
