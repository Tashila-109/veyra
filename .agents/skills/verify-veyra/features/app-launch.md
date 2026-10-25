# App launch

Status: passed, 2026-10-25, 11bf6ac, none (fake CLI RUN 8571f407), 496cd7268b5394857a28130629b0b4a75a897d0aa0ce2c43d5a4e41dc2814abe
Covers: crates/engine/src/data_dir.rs crates/veyra/src/ui/new_thread.rs

## Sub-features

- `launch-window`: one native window, Geist, sidebar, header and upper-third new-thread composer.
Read the empty-state element, trailing Add project and New thread icon buttons, and the gear-and-Settings foot aligned with the rows' inset.
- `launch-ready`: exact ready line with this run's pid and data folder while `login_path` is still `pending` in the fake lane.
- `launch-identity`: live pid, build revision, data folder, schema_version and system appearance.
- `launch-database`: migration 4, WAL, eight tables and Full access default.
- `launch-recovery`: the project written before the schema-2 relaunch survives it in the same database; [restart-recovery](restart-recovery.md) checks the quit and crash relaunches.
- `launch-lock`: a second process refuses the same folder while the first still answers.
- `launch-controls`: live width and notice transitions through actions and individual keys.
- `launch-log`: the ready line and rejected Send notice reach the application log.
- `launch-release`: no driver or provider override strings, ctl rejected, private MCP socket and no driver socket.
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
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
pid = identity()
dump = ctl('dump-before','dump')
launch = json.loads((PROOF/'launch-login-path.json').read_text())
assert launch['pid'] == pid and launch['data_dir'] == str(DATA)
assert f'veyra ready pid={pid} data_dir={DATA}' in (PROOF/'launch.stderr').read_text()
if LANE == 'fake':
    assert launch['login_path'] == 'pending'
assert dump['login_path'] in ['pending','loaded','fallback']
assert dump['sidebar']['width'] == 240 and dump['sidebar']['rows'] == []
assert dump['notice'] is None and dump['view'] == 'new_thread'
assert dump['drawer'] is None and dump['focus'] == 'composer'
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
page_done('app-launch-main')
print('PASS app-launch actions, individual keys, input handler and width clamps')
PY
```

### Migrated database and relaunch

The project write is the pre-quit value.
The helper uses one expected table list in SKILL.md.
The migration check seeds a stopped schema-2 database with a settled report, then verifies schema 4 adds a null source and the Translucent background default without inferring a badge; it removes only those fixture rows while stopped.
A missing or lost WAL row fails the project equality after the relaunch.
Relaunch updates the pid, group and window records before Doctor or Evidence.
The quit and crash relaunches, with the same WAL, inode, project-row and schema checks, run in [restart-recovery](restart-recovery.md).

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
identity()
action('persistent-project','AddProject',{'path':str(DATA/'project')})
poll('persistent-project-added',lambda d: any(row['kind']=='project' for row in d['sidebar']['rows']))
row = sql('select id,path,name from projects order by id')
assert row
(PROOF/'persisted-project.txt').write_text(row+'\n')
database('initial','full_access')
stop('before-migration')
import uuid
root,child,task,turn,wake,report,idle = [str(uuid.uuid4()) for _ in range(7)]
with sqlite3.connect(DB) as db:
    db.execute('pragma foreign_keys=on')
    db.execute('alter table settings drop column window_background')
    db.execute('alter table tasks drop column source_item_id')
    db.execute('pragma user_version=2')
    project = db.execute('select id from projects order by id').fetchone()[0]
    config = json.dumps({'model':None,'effort':None,'mode':'full_access'})
    for thread,parent,title in [(root,None,'Migration parent'),(child,root,'Migration child'),
            (idle,None,'Idle mark')]:
        db.execute("""insert into threads(id,project_id,title,parent_thread_id,relationship,root_thread_id,
            provider,config_json,cwd,view,import_pending,created_at)
            values(?,?,?,?,?,?,'claude',?,?,'chat',0,1)""",
            (thread,project,title,parent,'subagent' if parent else None,root if parent else thread,
                config,str(DATA/'project')))
    for identifier,thread,origin in [(turn,child,{'Task':{'task':task}}),(wake,root,{'Wake':{'tasks':[task]}})]:
        db.execute("""insert into turns(id,thread_id,ordinal,origin_json,state,outcome_json,queued_at,started_at,ended_at)
            values(?,?,1,?,'ended',?,1,1,2)""",(identifier,thread,json.dumps(origin),json.dumps('Completed')))
    db.execute("""insert into items(id,thread_id,turn_id,seq,kind,body_json,done)
        values(?,?,?,1,'assistant_text',?,1)""",
        (report,child,turn,json.dumps({'AssistantText':{'text':'Legacy report','done':True}})))
    db.execute("""insert into tasks(id,parent_thread_id,child_thread_id,turn_id,request_key,state,
        outcome_json,summary,delivery,wake_turn) values(?,?,?,?,?,'settled',?,'Legacy report','delivered',?)""",
        (task,root,child,turn,'migration report',json.dumps('Completed'),wake))
assert sql('pragma user_version') == '2'
start('after-migration')
action('migration-solid','SetWindowBackground',{'background':'solid'})
poll('migration-solid-applied',lambda d:d['look']['background']=='solid')
database('after-migration','full_access')
dump = poll('migration-task-loaded',lambda d: any(t['id']==task for t in d['tasks']))
settled = next(t for t in dump['tasks'] if t['id']==task)['state']['Settled']
assert settled['summary'] == 'Legacy report' and settled['source'] is None
assert 'Delivered' in settled['delivery']
assert sql(f"select source_item_id is null from tasks where id='{task}'") == '1'
action('app-idle-mark-select','SelectThread',{'thread':idle})
idle_view = poll('app-idle-mark',lambda d:d['selected_thread'] is not None
    and d['selected_thread']['id']==idle and d['selected_thread']['status']=='Idle')
assert idle_view['selected_thread']['status']=='Idle'
assert idle_view['transcript']['items']==[]
capture('app-idle-mark')
action('migration-select-child','SelectThread',{'thread':child})
poll('migration-transcript',lambda d: d['selected_thread'] and d['selected_thread']['id']==child
    and len(d['transcript']['items'])==1)
assert ctl('migration-unlinked-report','dump')['transcript']['items'][0]['report_delivered'] is False
capture('migration-unlinked-report')
stop('migration-fixture-cleanup')
with sqlite3.connect(DB) as db:
    db.execute('pragma foreign_keys=on')
    db.execute('delete from tasks where id=?',(task,))
    db.execute('delete from items where id=?',(report,))
    db.execute('delete from turns where id in (?,?)',(turn,wake))
    for thread in [child,root,idle]: db.execute('delete from threads where id=?',(thread,))
start('after-migration-cleanup')
assert sql('select count(*) from threads') == '0'
assert sql('select id,path,name from projects order by id') == row
assert (PROOF/'after-migration-inode.txt').read_text() == (PROOF/'initial-inode.txt').read_text()
print('PASS app-launch schema-2 task migrated with null source, no inferred badge and rows preserved')
page_done('app-launch')
PY
```

### Release check

Do not run it while another verification run is in restart-recovery's crash step.
A run selects this section through `release` (see [Selection](../SKILL.md#selection)), and every full run includes it.
After the debug app stops, the release launch refuses any running `veyra`.

Use a disposable HOME and a restricted PATH, then launch through macOS sandbox-exec with only the release Veyra executable allowed.
The profile denies every child executable, so release probes cannot start an installed provider even when its known install path exists.
The profile and provider-path metadata stay in release proof.
Release still reads VEYRA_DATA_DIR, but excludes the driver, recording and provider overrides at compile time.
The debug strings are the positive control.
Before launch, the shared in-memory KERN_PROCARGS2 reader refuses any same-user marked process whose VEYRA_PARENT_PID names a dead pid.
It prints and records only the count.
Release ignores VEYRA_REAP_PIDS, so its unrestricted startup sweep requires this preflight.
After cleanup, reject a release log containing an orphan-group signal, including one from an orphan created after preflight.
The cleanup trap owns only this release group and scratch folder.
This section stops the debug app before opening the release window, then relaunches debug after release cleanup.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
REL_DATA=${VEYRA_VERIFY_TMP:-/tmp/veyra-verify-}$RUN-rel
REL_PROOF=$PROOF/release
test ! -e "$REL_DATA"
mkdir -p "$REL_PROOF"
test -x /usr/bin/sandbox-exec
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
stop('before-release')
PY
cargo build --release -p veyra
if target/release/veyra ctl dump >"$REL_PROOF/ctl.txt" 2>&1; then rc=0; else rc=$?; fi
printf 'exit=%s\n' "$rc" >>"$REL_PROOF/ctl.txt"
test "$rc" = 2
grep -qx 'usage: veyra \[mcp\]' "$REL_PROOF/ctl.txt"
: >"$REL_PROOF/strings.txt"
for name in control.sock VEYRA_CLAUDE_BIN VEYRA_CODEX_BIN VEYRA_GROK_BIN VEYRA_FAKE_FIXTURES VEYRA_RECORD VEYRA_REAP_PIDS DebugNotificationResponse DebugDragSidebar DebugDoubleClickSidebar; do
    release=$(/usr/bin/strings -a target/release/veyra | grep -c "$name" || true)
    debug=$(/usr/bin/strings -a target/debug/veyra | grep -c "$name" || true)
    printf 'release %s count=%s\ndebug %s count=%s\n' "$name" "$release" "$name" "$debug" \
        | tee -a "$REL_PROOF/strings.txt"
    test "$release" = 0
    test "$debug" -gt 0
done
python3 - <<'PYPROFILE'
import json,os,pathlib
proof=pathlib.Path(os.environ['RUN'])
proof=pathlib.Path('target/verify-veyra')/proof/'release'
executable=str(pathlib.Path('target/release/veyra').resolve())
profile='(version 1)\n(allow default)\n(deny process-exec)\n(allow process-exec (literal '+json.dumps(executable)+'))\n'
(proof/'no-child-exec.sb').write_text(profile)
metadata={str(pathlib.Path(directory)/name):os.access(pathlib.Path(directory)/name,os.X_OK) for directory in ['/opt/homebrew/bin','/usr/local/bin','/usr/bin'] for name in ['claude','codex','grok']}
(proof/'provider-paths.json').write_text(json.dumps(metadata,indent=2)+'\n')
PYPROFILE
if /usr/bin/sandbox-exec -f "$REL_PROOF/no-child-exec.sb" target/release/veyra ctl dump >"$REL_PROOF/profile-load.txt" 2>&1; then rc=0; else rc=$?; fi
printf 'exit=%s\n' "$rc" >>"$REL_PROOF/profile-load.txt"
test "$rc" = 2
grep -qx 'usage: veyra \[mcp\]' "$REL_PROOF/profile-load.txt"
DEBUG_PID=$(<"$PROOF/pid")
if kill -0 "$DEBUG_PID" 2>/dev/null; then
    printf 'debug app is still running before release launch\n' >&2
    exit 1
fi
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
orphans = marked_processes()
(PROOF/'release'/'orphan-preflight.txt').write_text(f'marked orphan count={len(orphans)}\n')
print(f'marked orphan count={len(orphans)}')
assert not orphans, 'release launch refused: marked orphan processes exist'
PY
test "$(ioreg -n Root -d1 -a | grep -c CGSSessionScreenIsLocked || true)" = 0
! pgrep -x veyra >/dev/null
mkdir -p "$REL_DATA/home"
set -m
HOME="$REL_DATA/home" SHELL=/usr/bin/false PATH=/usr/bin:/bin:/usr/sbin:/sbin VEYRA_DATA_DIR="$REL_DATA" \
    /usr/bin/sandbox-exec -f "$REL_PROOF/no-child-exec.sb" target/release/veyra >"$REL_PROOF/stdout.log" 2>"$REL_PROOF/stderr.log" &
REL_PID=$!
REL_PGID=$REL_PID
printf '%s\n' "$REL_PID" >"$REL_PROOF/pid"
printf '%s\n' "$REL_PGID" >"$REL_PROOF/pgid"
stop_release() {
    if kill -0 "$REL_PID" 2>/dev/null; then
        actual=$(ps -o pgid= -p "$REL_PID" | tr -d ' ')
        test "$REL_PID" = "$REL_PGID" && test "$actual" = "$REL_PGID" || return 1
        kill -TERM -- "-$REL_PGID"
    fi
    for attempt in $(seq 1 50); do kill -0 "$REL_PID" 2>/dev/null || break; sleep .1; done
    if kill -0 "$REL_PID" 2>/dev/null; then kill -KILL -- "-$REL_PGID"; fi
    for attempt in $(seq 1 50); do kill -0 "$REL_PID" 2>/dev/null || break; sleep .1; done
    ! kill -0 "$REL_PID" 2>/dev/null || return 1
    test ! -f "$REL_DATA/logs/veyra.log" || cp "$REL_DATA/logs/veyra.log" "$REL_PROOF/veyra.log"
    test "$REL_DATA" = "${VEYRA_VERIFY_TMP:-/tmp/veyra-verify-}$RUN-rel" || return 1
    rm -rf "$REL_DATA"
    printf 'PASS release cleanup PID=%s DATA=%s\n' "$REL_PID" "$REL_DATA"
}
trap stop_release EXIT
test "$(ps -o pgid= -p "$REL_PID" | tr -d ' ')" = "$REL_PGID"
for attempt in $(seq 1 100); do
    grep -q "^veyra ready pid=$REL_PID data_dir=$REL_DATA$" "$REL_PROOF/stderr.log" && break
    kill -0 "$REL_PID" 2>/dev/null || break
    sleep .1
done
grep -q "^veyra ready pid=$REL_PID data_dir=$REL_DATA$" "$REL_PROOF/stderr.log"
/bin/ls -laR "$REL_DATA" >"$REL_PROOF/data-listing.txt"
test ! -e "$REL_DATA/control.sock"
test -S "$REL_DATA/veyra.sock"
test "$(stat -f %Lp "$REL_DATA/veyra.sock")" = 600
printf 'PASS release usage exit=2; driver and override strings absent; no driver socket\n'
for attempt in $(seq 1 100); do
    grep -q 'provider probe failed.*Operation not permitted\|provider probe failed.*Permission denied' "$REL_DATA/logs/veyra.log" && break
    sleep .1
done
stop_release
trap - EXIT
test -s "$REL_PROOF/veyra.log"
if grep -q 'reaper signalled orphan group' "$REL_PROOF/veyra.log"; then
    printf 'release check failed: startup signalled an orphan group\n' >&2
    exit 1
fi
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
import re
release = PROOF/'release'
metadata = json.loads((release/'provider-paths.json').read_text())
log = (release/'veyra.log').read_text()
probes = [line for line in log.splitlines() if 'provider probe failed' in line]
assert probes, 'release probe diagnostics absent'
denied = [line for line in probes if 'Operation not permitted' in line or 'Permission denied' in line]
assert not any(re.search(r'\b\d+\.\d+\.\d+\b',line) for line in probes), 'release probe returned a version'
assert all('not installed' in line or line in denied for line in probes), probes
if any(metadata.values()): assert denied, 'installed provider exec was not denied'
(release/'provider-probes.txt').write_text('\n'.join(probes)+'\n')
print('PASS release sandbox loaded; installed-provider exec denied; no provider version received')
start('after-release')
page_done('app-launch-release')
print('PASS debug relaunched after release window ended')
PY
```

## Observable success

- Window: inspect `window-launch.png` for the empty sidebar, left-aligned Add project, header, upper-third composer and Geist.
- Identity: Doctor asserts the live pid, revision, data folder, schema_version, system appearance and window id, within two seconds.
- Lock: `second-launch.txt` contains the refusal and exit 1; the first process still answers.
- Controls: `drive.log` records exit codes and all asserted width, text and notice transitions.
- Log: `veyra.log` retains the asserted ready line and rejected Send notice.
- Migration: `migration-task-loaded.json` retains the delivered summary with a null source; `migration-unlinked-report.png` shows the legacy reply without a delivery badge after the schema-2 to schema-4 launch.
- Recovery: `persisted-project.txt` matches the SQLite row after the schema-2 relaunch and its fixture cleanup; migration, WAL, tables, mode and inode are asserted.
- Release: `release/strings.txt` has zero release counts and positive debug controls for every driver and override name, `ctl.txt` exits 2 and `data-listing.txt` has no driver socket.
- Release: `release/provider-paths.json` records each known provider path, including `/opt/homebrew/bin/codex`; `profile-load.txt` proves the sandbox loaded.
- Release: `release/veyra.log` contains no `reaper signalled orphan group` line; `provider-probes.txt` shows failed execs and no provider version.
- Cleanup: `marker-check.txt` passes and DATA is absent while proof remains.

### Evidence contract

```json
{
  "required": {
    "app-launch-main": [
      "window-launch.png",
      "dump-before.json",
      "second-launch.txt"
    ],
    "app-launch": [
      "app-idle-mark.json",
      "app-idle-mark.png",
      "initial-database.txt",
      "after-migration-database.txt",
      "persisted-project.txt",
      "migration-unlinked-report.png"
    ],
    "app-launch-release": [
      "release/ctl.txt",
      "release/strings.txt",
      "release/data-listing.txt",
      "release/provider-probes.txt"
    ]
  },
  "pairs": {}
}
```

## Gotchas

The composer is now real; a text error is not expected while it is focused.
Adding a project and checking readiness run no turn.
This page leaves the fixture binary missing for new-thread's installation proof.
The native folder picker, pointer resizing and theme switching remain manual entry points.
The release sandbox must deny every child executable; do not fall back to an unsandboxed launch.
An unavailable sandbox-exec, an invalid profile or a missing exec-denial receipt fails the release check.
