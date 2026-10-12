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
For an authorized live lane, export `VEYRA_VERIFY_LANE=live` before Launch.
Launch then removes replay overrides, records to `$PROOF/rec`, and the new-thread and chat pages use their live branches.
A live lane needs separate human authorization because it runs the installed CLI and spends its quota.
The debug-only VEYRA_REAP_PIDS filter restricts startup reaping to the run's saved provider pids, including after a crash.
An empty list permits no startup signals.
The release check refuses to launch while any same-user marked process names a dead parent.
Its preflight and Cleanup share an in-memory marker reader and report counts only.
Both lanes remove the caller's Claude session variables while keeping `CLAUDE_CODE_USE_*` user configuration.
Every app launch runs from `$DATA/appcwd`; probes run in Veyra's private data folder.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=$(uuidgen | cut -c1-8 | tr A-Z a-z)
DATA=/tmp/veyra-verify-$RUN
PROOF=target/verify-veyra/$RUN
test ! -e "$DATA"
test ! -e "$PROOF"
test "$(ioreg -n Root -d1 -a | grep -c CGSSessionScreenIsLocked || true)" = 0
for attempt in $(seq 1 100); do
    if pgrep -x veyra >/dev/null; then sleep .1; else break; fi
done
! pgrep -x veyra >/dev/null
sleep 10
! pgrep -x veyra >/dev/null
cargo build --workspace
mkdir -p "$PROOF/fixtures" "$DATA/project" "$DATA/appcwd"
printf '%s\n' "${VEYRA_VERIFY_LANE:-fake}" >"$PROOF/lane"
: >"$PROOF/pages-run"
cat >"$PROOF/capture-window.swift" <<'SWIFT'
import AppKit
import CoreGraphics
import Foundation
let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 2, let pid = Int32(args[0]), let id = UInt32(args[1]) else { exit(2) }
func fail(_ text: String) -> Never {
    fputs(text + ". Veyra must stay frontmost during captures.\n", stderr)
    exit(1)
}
guard NSRunningApplication(processIdentifier: pid) != nil else { fail("owned app is gone") }
let end = Date().addingTimeInterval(2)
while NSWorkspace.shared.frontmostApplication?.processIdentifier != pid && Date() < end {
    RunLoop.current.run(until: Date().addingTimeInterval(0.05))
}
guard let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]],
      let window = windows.first(where: {
          ($0[kCGWindowNumber as String] as? UInt32) == id &&
          ($0[kCGWindowOwnerPID as String] as? Int32) == pid &&
          ($0[kCGWindowLayer as String] as? Int) == 0
      }), let bounds = window[kCGWindowBounds as String] as? [String: NSNumber],
      let x = bounds["X"], let y = bounds["Y"], let width = bounds["Width"], let height = bounds["Height"]
else { fail("owned window is off-screen") }
let frame = CGRect(x: x.doubleValue, y: y.doubleValue, width: width.doubleValue, height: height.doubleValue)
var count: UInt32 = 0
CGGetActiveDisplayList(0, nil, &count)
var displays = [CGDirectDisplayID](repeating: 0, count: Int(count))
CGGetActiveDisplayList(count, &displays, &count)
guard displays.contains(where: { frame.intersects(CGDisplayBounds($0)) })
else { fail("owned window is outside the active displays") }
guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid
else { fail("owned window is covered by another application") }
SWIFT
swiftc -O "$PROOF/capture-window.swift" -o "$PROOF/capture-window"
cp crates/engine/tests/fixtures/claude/probe-initialize.jsonl "$PROOF/fixtures/probe.jsonl"
printf 'RUN=%s\n' "$RUN"
cat >"$PROOF/driver.py" <<'PY'
import ctypes, errno, hashlib, json, os, pathlib, re, shutil, signal, sqlite3, struct, subprocess, sys, time
PROOF = pathlib.Path(__file__).resolve().parent
RUN = os.environ['RUN']
DATA = pathlib.Path('/tmp/veyra-verify-' + RUN)
DB = DATA / 'veyra.db'
FIXTURES = PROOF / 'fixtures'
SOURCE = pathlib.Path('crates/engine/tests/fixtures/claude')
TABLES = 'grants items projects requests settings tasks threads turns'.split()
LANE = (PROOF/'lane').read_text().strip()
assert LANE in ['fake','live'], LANE
ENV = {key:value for key,value in os.environ.items() if not (
    (key.startswith('CLAUDE_CODE_') and not key.startswith('CLAUDE_CODE_USE_'))
    or key in ['CLAUDE_PID','CLAUDE_EFFORT','CLAUDECODE','CLAUDE_AGENT_SDK_VERSION'])}
ENV['VEYRA_DATA_DIR'] = str(DATA)
ENV['VEYRA_REAP_PIDS'] = ''
for name in ['VEYRA_CLAUDE_BIN','VEYRA_FAKE_FIXTURES','VEYRA_FAKE_FIXTURE','VEYRA_RECORD']:
    ENV.pop(name,None)
if LANE == 'fake':
    ENV.update(VEYRA_CLAUDE_BIN=str(DATA/'fake-cli'),VEYRA_FAKE_FIXTURES=str(FIXTURES.resolve()))
else:
    ENV['VEYRA_RECORD'] = str(PROOF.resolve()/'rec')
lib = ctypes.CDLL(None,use_errno=True)
lib.sysctl.argtypes = [ctypes.POINTER(ctypes.c_int),ctypes.c_uint,ctypes.c_void_p,
    ctypes.POINTER(ctypes.c_size_t),ctypes.c_void_p,ctypes.c_size_t]
def no_environment(pid):
    status = subprocess.run(['ps','-o','stat=','-p',str(pid)],capture_output=True,text=True).stdout.strip()
    return not status or status.startswith('Z')
def marked_processes(parents=None):
    remaining = []
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
            if error == errno.EINVAL:
                remaining.append(pid)
                continue
            raise OSError(error,f'marker size read failed for pid {pid}')
        if size.value == 0: continue
        buffer = ctypes.create_string_buffer(size.value)
        if lib.sysctl(mib,3,buffer,ctypes.byref(size),None,0) != 0:
            error = ctypes.get_errno()
            if error == errno.ESRCH or (error == errno.EINVAL and no_environment(pid)): continue
            if error == errno.EINVAL:
                remaining.append(pid)
                continue
            raise OSError(error,f'marker read failed for pid {pid}')
        raw = buffer.raw[:size.value]
        if not raw: continue
        try:
            argc = struct.unpack_from('i',raw)[0]
            if argc < 0: raise ValueError('invalid argument count')
            offset = raw.index(b'\0',4)+1
            while offset < len(raw) and raw[offset] == 0: offset += 1
            for _ in range(argc): offset = raw.index(b'\0',offset)+1
        except (ValueError,struct.error):
            if not no_environment(pid): remaining.append(pid)
            continue
        markers = [value.split(b'=',1)[1] for value in raw[offset:].split(b'\0')
            if value.startswith(b'VEYRA_PARENT_PID=')]
        for value in markers:
            if not value.isdigit() or int(value) <= 0: continue
            parent = int(value)
            if parents is not None:
                orphan = parent in parents
            else:
                try: os.kill(parent,0)
                except ProcessLookupError: orphan = True
                else: orphan = False
            if orphan:
                remaining.append(pid)
                break
    return remaining
def saved_marked_processes(marked, pids, groups):
    owned = []
    for pid in marked:
        try: group = os.getpgid(pid)
        except ProcessLookupError: continue
        if pid in pids or group in groups: owned.append(pid)
    return owned
def page_done(name):
    with (PROOF/'pages-run').open('a') as out: out.write(name+'\n')
def save(name, value):
    (PROOF / (name + '.json')).write_text(json.dumps(value, indent=2) + '\n')
    return value
def ctl(name, *args, expected=0, stderr_contains=None):
    result = subprocess.run(['target/debug/veyra', 'ctl', *args], env=ENV, capture_output=True, text=True)
    with (PROOF/'drive.log').open('a') as log:
        log.write('$ veyra ctl ' + ' '.join(args) + '\n'
            + result.stdout + result.stderr + f'exit={result.returncode}\n')
    (PROOF/(name+'.json')).write_text(result.stdout)
    (PROOF/(name+'.stderr')).write_text(result.stderr)
    assert result.returncode == expected, (name, expected, result.returncode, result.stdout, result.stderr)
    if expected == 1: assert stderr_contains, 'exit 1 needs a named stderr expectation'
    if stderr_contains: assert stderr_contains in result.stderr, (name,result.stderr,stderr_contains)
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
def window_id(pid):
    end = time.monotonic() + 30
    while time.monotonic() < end:
        found = subprocess.run(['swift','.agents/skills/verify-veyra/bin/winid.swift',str(pid)],
            capture_output=True,text=True)
        if found.returncode == 0: return found.stdout.strip()
        time.sleep(.05)
    raise AssertionError('no on-screen window for owned pid '+str(pid))
def capture(name):
    assert subprocess.run(['ioreg','-n','Root','-d1','-a'],capture_output=True,text=True).stdout.count('CGSSessionScreenIsLocked') == 0
    window = (PROOF/'window-id').read_text().strip()
    pid = (PROOF/'pid').read_text().strip()
    helper = str((PROOF/'capture-window').resolve())
    for step in range(2):
        frame = ctl(name+'-before-frame-'+str(step),'dump')['perf']['repaints']
        action(name+'-activate-'+str(step),'ActivateWindow')
        poll(name+'-rendered-'+str(step),lambda d: d['perf']['repaints'] > frame)
    time.sleep(.1)
    subprocess.run([helper,pid,window],check=True)
    dump = ctl(name+'-capture-state','dump')
    visible = {key:dump[key] for key in ['view','sidebar','selected_thread','transcript','composer_text',
        'composer','settings','notice','quit_sheet']}
    visible['providers'] = [probe['readiness'] for probe in dump['providers']]
    state = json.dumps(visible,sort_keys=True)
    target = PROOF/(name+'.png')
    subprocess.run(['screencapture','-x','-o','-l',window,str(target)],check=True)
    subprocess.run([helper,pid,window],check=True)
    digest = hashlib.sha256(target.read_bytes()).hexdigest()
    previous = PROOF/'capture-last.json'
    if previous.exists():
        old = json.loads(previous.read_text())
        assert state == old['state'] or digest != old['digest'], 'stale screenshot after state change: '+name
    previous.write_text(json.dumps({'state':state,'digest':digest,'name':name})+'\n')
def foreground_ended(name, thread):
    query = "select count(*) from turns where thread_id='"+thread+"' and state='ended'"
    end = time.monotonic()+30
    while time.monotonic()<end:
        if sql(query)=='1': return
        time.sleep(.05)
    raise AssertionError(name+' foreground did not end')
def background_working(name, thread):
    foreground_ended(name,thread)
    ctl(name+'-not-done','wait',thread,'Done','1',expected=1,stderr_contains='veyra ctl wait timed out:')
    busy = ctl(name,'dump')
    assert busy['selected_thread']['id']==thread and busy['selected_thread']['status']=='Working'
    return busy
def sql(query):
    value = subprocess.check_output(['sqlite3',str(DB),query],text=True).strip()
    with (PROOF/'sql.log').open('a') as log: log.write(query+'\n'+value+'\n')
    return value
def database(label, mode):
    values = [sql('pragma user_version'),sql('pragma journal_mode'),
        *sql('.tables').split(),sql('select default_mode from settings')]
    assert values == ['2','wal',*TABLES,mode], (label,values)
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
def stop(label, crash=False):
    pid = identity()
    live = sql('select count(*) from threads where live_pgid is not null')
    assert live == '0', 'finish provider turns before relaunch'
    if crash: os.kill(pid,signal.SIGKILL)
    else: ctl(label+'-quit','quit')
    wait_gone(pid)
def start(label):
    assert subprocess.run(['ioreg','-n','Root','-d1','-a'],capture_output=True,text=True).stdout.count('CGSSessionScreenIsLocked') == 0
    quiet = time.monotonic()
    deadline = quiet + 30
    while time.monotonic()-quiet < 10:
        assert time.monotonic() < deadline, "another Veyra prevents a quiet launch"
        if subprocess.run(['pgrep','-x','veyra'],capture_output=True).returncode == 0:
            quiet = time.monotonic()
        time.sleep(.1)
    ENV['VEYRA_REAP_PIDS'] = ','.join(sql('select live_pid from threads where live_pid is not null').split())
    app = subprocess.Popen([str(pathlib.Path('target/debug/veyra').resolve())],cwd=DATA/'appcwd',env=ENV,
        stdout=(PROOF/(label+'.stdout')).open('w'),stderr=(PROOF/(label+'.stderr')).open('w'),
        start_new_session=True)
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
    window = window_id(pid)
    assert window
    (PROOF/'window-id').write_text(window+'\n')
    identity()
    return pid
def restart(label, crash=False):
    stop(label,crash)
    return start(label)
def stage_records(records):
    text = '\n'.join(json.dumps(record) for record in records)+'\n'
    for n in range(1,int(sql('select count(*) from threads'))+2):
        for epoch in range(1,int(sql('select count(*) from turns'))+2):
            (FIXTURES/f't{n}-s{epoch}.jsonl').write_text(text)
def stage(scenario, slow=False, turns=1):
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
    if scenario == 'background-tasks':
        for record in records:
            wire = record.get('line',{})
            ambient_only = wire.get('tasks') == [{'id':'ambient-work','ambient':True}]
            if wire.get('subtype') == 'background_tasks_changed' and ambient_only:
                record['after_ms'] = 3000
            if wire.get('subtype') == 'background_tasks_changed' and wire.get('tasks') == []:
                record['after_ms'] = 3000
    if turns > 1:
        assert scenario == 'ok-reply'
        start = next(i for i,record in enumerate(records) if record.get('type')=='expect_outbound'
            and record.get('line',{}).get('type')=='user')
        repeat = records[start:-1]
        ending = records[-1]
        records = records[:-1]
        for turn in range(2,turns+1):
            text = json.dumps(repeat).replace('<id:prompt>', '<id:prompt'+str(turn)+'>')
            records.extend(json.loads(text))
        records.append(ending)
    stage_records(records)
def create(name, prompt, slow=False, scenario='ok-reply'):
    if LANE == 'fake' and scenario is not None: stage(scenario,slow)
    action(name+'-new','NewThread')
    assert ctl(name+'-text','text',prompt) == {'ok':True}
    before = int(sql('select count(*) from threads'))
    ctl(name+'-send','keys','secondary-enter')
    dump = poll(name+'-selected',lambda d: d['selected_thread'] is not None
        and int(sql('select count(*) from threads')) == before+1)
    return dump['selected_thread']['id']
def stage_family(parent='waiting', child='complete'):
    pid = identity()
    counter = PROOF/'family-lanes.json'
    lanes = json.loads(counter.read_text()) if counter.exists() else {}
    number = lanes.get('next',1) if lanes.get('pid') == pid else 1
    counter.write_text(json.dumps({'pid':pid,'next':number+2})+'\n')
    records = [json.loads(line) for line in (SOURCE/'parent-start-thread.jsonl').read_text().splitlines()]
    first = next(i for i,record in enumerate(records) if record.get('line',{}).get('type') == 'result')
    if parent == 'running':
        records.insert(first, {'type':'expect_outbound','line':{'type':'control_request','request_id':'veyra-interrupt','request':{'subtype':'interrupt'}}})
        records[first+1]['after_ms'] = 0
        records[first+1]['line']['subtype'] = 'error_during_execution'
    elif parent == 'attention':
        approval = [json.loads(line) for line in (SOURCE/'approval-deny.jsonl').read_text().splitlines()]
        insertion = next(i for i,record in enumerate(records) if record.get('type') == 'mcp_expect' and record.get('line',{}).get('id') == 3)+1
        records[insertion:insertion] = approval[4:6]
    else:
        assert parent == 'waiting'
    if child == 'complete':
        child_records = [json.loads(line) for line in (SOURCE/'child-task.jsonl').read_text().splitlines()]
        for record in child_records:
            wire = record.get('line',{})
            if wire.get('type') == 'assistant':
                record['after_ms'] = 2000
                wire['message']['content'][0]['text'] = 'First report line\n**Second report line**\nThird report line\n\n- A list item\n- Another item\n\n~~~text\ncode line one\ncode line two\n~~~'
            if wire.get('type') == 'result': record['after_ms'] = 30000
    else:
        assert child == 'running'
        child_records = [json.loads(line) for line in (SOURCE/'long-reply.jsonl').read_text().splitlines()]
    def write(lane,epoch,records):
        (FIXTURES/f't{lane}-s{epoch}.jsonl').write_text('\n'.join(json.dumps(record) for record in records)+'\n')
    write(number,1,records)
    write(number+1,1,child_records)
    resumed = [json.loads(line) for line in (SOURCE/'parent-wake.jsonl').read_text().splitlines()]
    for epoch in [2,3]: write(number,epoch,resumed)
    return number
def create_family(name, parent='waiting', child='complete'):
    stage_family(parent,child)
    action(name+'-new','NewThread')
    ctl(name+'-text','text',name)
    before = int(sql('select count(*) from threads'))
    ctl(name+'-send','keys','secondary-enter')
    dump = poll(name+'-root',lambda d:d['selected_thread'] is not None
        and d['selected_thread']['lineage']=='Root' and d['selected_thread']['title']==name
        and int(sql('select count(*) from threads'))>=before+1)
    root = dump['selected_thread']['id']
    poll(name+'-task',lambda d:any(task['parent']==root for task in d['tasks']))
    row = sql("select child_thread_id||'|'||id from tasks where parent_thread_id='"+root+"'").split('|')
    assert len(row)==2
    return root,row[0],row[1]
def select(name, thread):
    action(name+'-select','SelectThread',{'thread':thread})
    return poll(name,lambda d:d['selected_thread'] is not None and d['selected_thread']['id']==thread
        and len(d['transcript']['items'])>0)
def family_delivered(name, root, task):
    return poll(name,lambda d:sql("select delivery from tasks where id='"+task+"'")=='delivered'
        and any(row['id']==root and row['status']=='Done' for row in d['sidebar']['rows'] if row['kind']=='thread'))
def pending_wake(root):
    return sql("select count(*) from tasks where parent_thread_id='"+root+"' and delivery='pending'")=='1'
PY
RUN="$RUN" PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
app = subprocess.Popen([str(pathlib.Path('target/debug/veyra').resolve())],cwd=DATA/'appcwd',env=ENV,
    stdout=(PROOF/'stdout.log').open('w'),stderr=(PROOF/'stderr.log').open('w'),start_new_session=True)
pid = app.pid
(PROOF/'pid').write_text(str(pid)+'\n')
(PROOF/'pgid').write_text(str(os.getpgid(pid))+'\n')
(PROOF/'owned-app-pids').write_text(str(pid)+'\n')
PY
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
test "$PID" = "$PGID"
for attempt in $(seq 1 100); do
    grep -q "^veyra ready pid=$PID data_dir=$DATA$" "$PROOF/stderr.log" && break
    kill -0 "$PID" 2>/dev/null || break
    sleep .1
done
grep -q "^veyra ready pid=$PID data_dir=$DATA$" "$PROOF/stderr.log"
ID=$(RUN="$RUN" PYTHONPATH="$PROOF" python3 -c 'from driver import *; print(window_id(int((PROOF/"pid").read_text())))')
test -n "$ID"
printf '%s\n' "$ID" >"$PROOF/window-id"
printf 'PASS launch RUN=%s PID=%s PGID=%s ID=%s\n' "$RUN" "$PID" "$PGID" "$ID"
```

The block must exit 0.
A failed launch may still own a process: run Cleanup before reporting.
Relaunches use `start_new_session=True`, preserving the same pid-equals-group invariant without a previous shell's job table.
The slowed `ok` fixture uses 500 ms, the design's stderr-wait bound, between its two text deltas and before the result.
The background proof uses three-second intervals, the design's interrupt grace, before ambient-only and empty lists.
The family proof delays its child reply by two seconds so selection precedes the text batch and the visible counter can observe it.
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
window = window_id(pid)
assert window == (PROOF/'window-id').read_text().strip()
bounds = subprocess.check_output(['swift','.agents/skills/verify-veyra/bin/winid.swift',str(pid),'--bounds'],text=True)
(PROOF/'bounds.txt').write_text(bounds)
print(f'PASS doctor pid={pid} appearance={expected} seconds={seconds:.3f} window={window}')
PY
```

## Drive

Run [app-launch](features/app-launch.md), then [new-thread](features/new-thread.md), [chat-send-stream](features/chat-send-stream.md) and [settings](features/settings.md).
Also run [approvals-questions](features/approvals-questions.md), [stop-interrupt](features/stop-interrupt.md) and [restart-recovery](features/restart-recovery.md).
Then run [multi-agent](features/multi-agent.md) for the thread tree, family actions, report delivery and quiet hint.
The fake lane runs those pages without a provider login.
[permission-matrix](features/permission-matrix.md) requires separately authorized live providers.
Use the same exported `RUN` in every fresh shell.
`action NAME [JSON]` builds a registered gpui action.
AllowOnce, AllowInThread and Deny accept a request id; AnswerQuestion accepts request, question index and option index.
Stop accepts a target containing thread and turn ids; without a payload it uses the rendered running or pending-admission turn.
Without a request payload, consent actions use the rendered dock target, as focused keyboard chords do.
For an explicit AnswerQuestion request, supply its question index as well.
Action and key replies acknowledge dispatch; poll `dump` for the resulting view or use `ctl wait` for a thread state.
`keys KEYSTROKES` dispatches each key through the window.
`text STRING` uses the focused textarea's public input handler; Settings has no text handler.
Enter or ⌘↵ sends through `veyra::Send`; Shift+Enter inserts a newline.
`wait THREAD STATUS [SECONDS]` is a client-only loop over `dump`, polling every 50 ms for 30 seconds by default.
It exits 0 for the status, 1 for timeout or a driver/connection error, 2 for bad usage and 3 for a missing or archived thread.
A timeout writes `veyra ctl wait timed out:` to stderr.
A connection or driver failure writes `veyra ctl driver error:` with its cause.
Every exit-1 expectation must also assert its named stderr line.
`quit` dispatches `veyra::ConfirmQuit`; `veyra::Quit` opens the sheet while work runs.
A timeout does not cancel an enqueued action; run Doctor before retrying.
Commands remain bounded at 64 KiB; the client accepts replies up to 4 MiB so the seeded 5,000-item transcript can be measured without truncating its dump.
After another Send on an existing thread, gate `wait Done` on the new turn row or a changed status so the preceding Done cannot satisfy it.

The read-only dump reports `pid`, `build_rev`, `data_dir`, `schema_version`, `appearance`, `view`, `header_branch`, `sidebar`, `selected_thread`, `transcript`, `composer_text`, `composer`, `providers`, `settings`, `perf`, `awake`, `processes`, `window_active`, `notifications`, `quit_sheet`, `requests` and `notice`.
`header_branch` is the branch text used by the header and is empty after leaving Chat.
`view` is `new_thread`, `chat`, `failed_thread` or `settings`.
`sidebar.rows` holds project and thread rows in tree order, with status for each thread, row_status for its rendered family glyph, depth, quiet_minutes and archive_keep_children.
`sidebar.menu_thread` names the open row menu.
OpenThreadMenu, Archive and ArchiveKeepChildren accept a thread id; the latter keeps a root's children running.
Each thread and selected_thread has lineage, and top-level tasks carries task state and delivery.
The child breadcrumb includes its root title and whether its SelectThread link is available; parent_attention names a parent that needs you.
`transcript.family_card` carries the first derived list row, Started N agents, and its Open and Stop actions.
`transcript.items` contains the stored item rows; item_tops includes the family card first when present.
Task and Wake rows carry turn_label and task_card text; report_delivered marks the assistant row whose text the delivered report carried.
`selected_thread.quiet_minutes` and header_status share Quiet for N min without changing status.
`selected_thread` includes its native id, configuration, failure cause and `header_status`, the rendered status with elapsed seconds.
`composer` includes the chosen provider and permission mode, plus Send's readiness and disabled reason.
`providers` reports each probe's path, version, models, readiness and `account`, either `known` or `unknown`, with no account value.
`transcript.items` carries the rendered items and `transcript.follow` is true while attached to the bottom.
`transcript.gap`, `top_item`, `top_offset` and `item_tops` expose the rendered ListState geometry for the follow and expansion checks.
Items include `outcome`, queued `wait_reason` and a tool's `failed_last_line`.
`selected_thread.hold` and `wait_reason` describe held work.
`composer.discard_queued_visible` reports the held queue control.
`composer.stop_visible` describes the empty running composer; `dock` holds its `request`, `position`, `count`, `focused` and `question_index`.
Approval answer chords appear only with dock focus; otherwise its hint shows the FocusDock chord.
Approval dock action_text, consequence and preview report the same typed target, one-line effect and bounded arguments rendered in the window.
A cut preview ends with a notice.
The full command or file path, consequence and bounded argument preview share a scrollable area within the composer height limit.
Allow and Deny stay below that area.
Questions share the same unfocused FocusDock prompt.
Answering one request keeps dock focus while requests remain.
`notifications.posted` counts banner requests; `quit_sheet.open` and `threads` describe the quit sheet.
Each transcript item carries `delta_count`, the actual text-stream deltas observed during this app run.
`composer.provider_picker_open` reports the picker opened by `OpenProviderPicker` or ⌘⌥⇧P.
`awake` reports the engine's idle-sleep guard.
`processes` lists each reported live CLI with its thread, pid and resident bytes, retaining retiring groups until Exited.
`perf.commits`, `perf.actor_max_ms` and `perf.actor_busy_ms` expose committed transactions and actor timing.
`perf.mailbox_free` is the engine mailbox's unreserved capacity; slots assigned to waiting sends count as used, so it reads 0 while any send waits.
Engine commands await space in the window's gpui task, so Stop is retained when that capacity is zero.
`perf.repaints` counts window renders and `perf.max_batch_events` counts the largest applied pump batch.
`perf.visible` holds `thread`, `applied_batches` and `max_batch_gap_ms` for the selected Chat thread.
Its count and largest gap reset on selection changes and on that thread's TurnStarted; hidden text changes neither counter.
`perf.transcript_open_ms` stamps the first render of the installed snapshot, measured from its Snapshot request.
It is null until a transcript opens, and each open replaces it.
The debug-only `veyra::DebugNotificationResponse` action takes a banner `tag` and invokes the native response handler.
The debug-only `veyra::DebugStallUi` action takes `{"milliseconds":2000}` and blocks the main thread for that interval.
`settings.default_mode` is the stored default; `perf.applied_batches` and `perf.max_batch_gap_ms` report the event pump.
Before every capture, the debug-only `ActivateWindow` action activates Veyra from inside the app.
The helper waits for two separate rendered frames and 100 ms before inspecting and capturing the window.
Capture checks the owned window is on-screen and verifies it remains frontmost.
Veyra must stay frontmost during captures.
A changed visible state with an identical PNG fails; Evidence also compares named state pairs.
Concurrent windows may cause a capture to fail, but cannot make a covered or off-screen frame pass.
A dump proves state, not layout; inspect each PNG.

## Evidence

Run this after the chosen pages and the final Doctor.
Each passed page records its name in `pages-run`; Evidence checks only those pages.
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
pages = set((PROOF/'pages-run').read_text().split())
assert pages, 'no completed page'
database('final','ask_first' if pages & {'settings','approvals-questions'} else 'full_access')
capture('window')
shutil.copy2(DATA/'logs/veyra.log',PROOF/'veyra.log')
(PROOF/'git-identity.txt').write_text(subprocess.check_output(['git','rev-parse','HEAD'],text=True)
    +subprocess.check_output(['git','status','--short'],text=True))
required = {
    'app-launch':['window-launch.png','dump-before.json','second-launch.txt','initial-database.txt',
        'after-quit-database.txt','after-crash-database.txt','persisted-project.txt','release/ctl.txt',
        'release/strings.txt','release/data-listing.txt'],
    'new-thread':['new-thread-missing.json','new-thread-ready.json','new-thread-no-account.json',
        'new-thread-provider-menu.png','new-thread-provider-menu-ready.png','new-thread-ready.png','new-thread.png'],
    'chat-send-stream':['chat-working.json','chat-working.png','chat-done.json','chat-done.png',
        'chat-failed.json','chat-failed.png','chat-provider-done.json','chat-background-working.json'],
    'approvals-questions':['approval-deny.png','approval-allow.png','question.png','question-two.png',
        'approvals-done.json','questions-done.json','approval-composer-pending.json','question-focused.png',
        'approval-deny-focused.png','approval-allow-focused.png','question-two-next.png',
        'approval-write-inside.png','approval-write-outside.png','approval-web-fetch.png',
        'approval-notebook-edit.png','approval-mcp.png','approval-long-command.png'],
    'stop-interrupt':['stop-working.png','stop-held.png','stop-discarded.json','stop-expired.json',
        'notifications-background.json','notifications-older-selected.json','notifications-older-response.png'],
    'restart-recovery':['restart-quit-sheet.png','restart-held.png','restart-crash-held.json','restart-continued.json',
        'restart-child-interrupted.png','restart-child-parent-held.png','restart-child-delivered.json'],
    'multi-agent':['multi-tree-working.png','multi-child-task.png','multi-family-done.png','multi-report-delivered.png',
        'multi-child-parent-needs.png','multi-menu.png','multi-child-stop-held.png','multi-parent-stop-held.png',
        'multi-child-archive-held.json','multi-child-archive-held.png',
        'multi-archived-family.json','multi-kept-child.png','multi-quiet-sidebar.png','multi-quiet-header.png'],
    'settings':['settings-first.json','settings-ask-first.json','settings-relaunch.json','settings.png',
        'settings-existing.json','settings-new.json'],
    'new-thread-live':['new-thread-live.json','new-thread-live.png'],
    'chat-send-stream-live':['chat-working.json','chat-working.png','chat-done.json','chat-done.png','rec/probe.jsonl'],
}
assert pages <= required.keys(), pages
for name in ['drive.log','sql.log',*(name for page in pages for name in required[page])]:
    assert (PROOF/name).stat().st_size > 0, name
if 'app-launch' in pages:
    assert len({(PROOF/(label+'-inode.txt')).read_text() for label in ['initial','after-quit','after-crash']}) == 1
for pid in (PROOF/'owned-app-pids').read_text().split():
    assert f'veyra ready pid={pid} data_dir={DATA}' in (PROOF/'veyra.log').read_text()
pairs = {
    'new-thread':[('new-thread-missing','new-thread-ready'),
        ('new-thread-provider-menu','new-thread-provider-menu-ready')],
    'chat-send-stream':[('chat-working','chat-done'),('chat-tool-collapsed','chat-tool-expanded')],
    'settings':[('settings-full-access','settings')],
    'chat-send-stream-live':[('chat-working','chat-done')],
}
for page in pages:
    for first,second in pairs.get(page,[]):
        assert (PROOF/(first+'.png')).read_bytes() != (PROOF/(second+'.png')).read_bytes(), (first,second)
if 'chat-send-stream-live' in pages:
    assert list((PROOF/'rec').glob('t*-s*.jsonl')), 'missing live session recording'
print('PASS evidence pages='+','.join(sorted(pages))+'; identity, database, relaunches, screenshots and release kept')
PY
```

Inspect the PNGs themselves for Geist, the 240 px initial sidebar, 44 px header, upper-third new-thread block, centred chat column and composer, collapsed tools/reasoning, failure cause and Settings copy.
The transcript caps at 752 px, the composer at 784 px, with 40 px gutters.
Yabai can override the requested 1280 by 800 outer bounds; use Doctor's actual bounds.
Do not count native typing, pointer dragging, IME or a manual theme change as driver proof.

## Cleanup

Read `live_pgid` before quitting or removing anything.
Quit the app, then send SIGTERM to saved provider groups and SIGKILL one second later.
Signal only groups saved in this run's database and the app group this run started.
The leftover checks retry for five seconds and fail only for saved run-owned pids or groups.
Other processes carrying the parent marker are allowed up to ten seconds to drain; any remaining marker then fails Cleanup, regardless of its group, with counts and pids only.
The marker check uses Darwin's `KERN_PROCARGS2` through `sysctl` in memory, never printing arguments or environment values.
It reads same-user parent markers in memory, then counts only saved app/provider pids and members of saved provider groups as owned leftovers.
macOS refuses argument reads for privileged processes whose real uid is the user but effective uid is root; a fake CLI lane cannot create such a process.
Zombies have no argument/environment block on Darwin and are excluded before the read; owned app pids must still be absent.
ESRCH or an empty argument block after either sysctl read means the process is gone.
EINVAL or an incomplete block from a still-listed process keeps it in the leftover count and retries within the same five-second window.
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
sqlite3 "$DATA/veyra.db" 'select live_pid from threads where live_pid is not null' >"$PROOF/provider-pids.txt"
test ! -f "$DATA/logs/veyra.log" || cp "$DATA/logs/veyra.log" "$PROOF/veyra.log"
if kill -0 "$PID" 2>/dev/null; then
    test "$(ps -o pgid= -p "$PID" | tr -d ' ')" = "$PGID"
    VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl quit >"$PROOF/quit.json" 2>"$PROOF/quit.stderr" || true
fi
while read -r group; do
    test -z "$group" || kill -TERM -- "-$group" 2>/dev/null || true
done <"$PROOF/provider-groups.txt"
sleep 1
while read -r group; do
    test -z "$group" || kill -KILL -- "-$group" 2>/dev/null || true
done <"$PROOF/provider-groups.txt"
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
PYTHONPATH="$PROOF" python3 - "$PROOF" <<'PY'
import ctypes, errno, json, os, pathlib, struct, subprocess, sys, time
proof = pathlib.Path(sys.argv[1])
parents = {int(pid) for pid in (proof/'owned-app-pids').read_text().split()}
groups = {int(group) for group in (proof/'provider-groups.txt').read_text().split()}
def groups_gone():
    remaining = []
    for group in groups:
        try: os.killpg(group,0)
        except ProcessLookupError: continue
        remaining.append(group)
    return remaining
from driver import marked_processes, saved_marked_processes
pids = parents | {int(pid) for pid in (proof/'provider-pids.txt').read_text().split()}
end = time.monotonic()+5
while True:
    remaining_groups = groups_gone()
    marked = marked_processes(parents)
    remaining_markers = saved_marked_processes(marked, pids, groups)
    with (proof/'leftover-checks.jsonl').open('a') as out:
        out.write(json.dumps({'groups':len(remaining_groups),'markers':len(remaining_markers),
            'time':time.monotonic()})+'\n')
    if not remaining_groups and not remaining_markers: break
    assert time.monotonic()<end, (remaining_groups,remaining_markers)
    time.sleep(.05)
started = time.monotonic()
while True:
    other_markers = marked_processes(parents)
    if not other_markers or time.monotonic()-started >= 10: break
    time.sleep(.05)
(proof/'hook-drain.json').write_text(json.dumps({'remaining':len(other_markers),
    'elapsed_seconds':time.monotonic()-started})+'\n')
assert not other_markers, f'{len(other_markers)} marked processes remain after the ten-second drain: {other_markers}'
(proof/'marker-check.txt').write_text(f'PASS no saved owned pids or provider groups for app pids {sorted(parents)}\n')
print('PASS saved owned pids, provider groups and marker check; other marker drain seconds='+str(round(time.monotonic()-started,3)))
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

### Gotchas

A live provider's session-end hook chains have been observed carrying the parent marker for up to nine seconds after quit or reap.
Cleanup signals only saved run-owned pids and groups, while other marked processes get a separate ten-second drain wait and fail Cleanup if they remain afterward.
The drain wait adds one second of margin to that observed chain lifetime, records counts and elapsed time, and never prints arguments or environments.
It does not signal an unrecorded group.

## Helpers

`bin/winid.swift PID` finds the on-screen window by owner pid and normal window layer.
Add `--bounds` to get `x,y,w,h`.
A socket path permits 103 usable bytes; keep RUN short.

## Maintain the map

Update the matching feature page when actions, commands or dump fields change.
`verify_map_names` checks exact action names, ctl verbs and top-level dump keys.
Update the `Status: STATE, DATE, COMMIT, MISSING` values from candidate acceptance in a docs-only change after the candidate commit.
